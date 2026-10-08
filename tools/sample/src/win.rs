//! Windows 上的进程采样。
//!
//! Private Bytes、工作集、句柄、USER/GDI、CPU 时间来自进程 API。
//! 每秒唤醒次数是 `\Thread(<进程名>*)\Context Switches/sec` 里属于该 PID 的线程之和。
//! 不调用 `EmptyWorkingSet`，也不提供裁剪工作集的参数。

use std::collections::HashMap;
use std::io::{self, Write};
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, FILETIME, HANDLE, SetLastError};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Performance::{
    PDH_CALC_NEGATIVE_DENOMINATOR, PDH_CALC_NEGATIVE_TIMEBASE, PDH_CALC_NEGATIVE_VALUE,
    PDH_CSTATUS_INVALID_DATA, PDH_CSTATUS_ITEM_NOT_VALIDATED, PDH_CSTATUS_NEW_DATA,
    PDH_CSTATUS_NO_INSTANCE, PDH_CSTATUS_VALID_DATA, PDH_FMT, PDH_FMT_COUNTERVALUE_ITEM_W,
    PDH_FMT_DOUBLE, PDH_FMT_LARGE, PDH_HCOUNTER, PDH_HQUERY, PDH_INVALID_DATA, PDH_MORE_DATA,
    PDH_NO_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
    PdhGetFormattedCounterArrayW, PdhOpenQueryW,
};
use windows::Win32::System::ProcessStatus::{
    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
};
use windows::Win32::System::Threading::{
    GR_GDIOBJECTS, GR_USEROBJECTS, GetExitCodeProcess, GetGuiResources, GetProcessHandleCount,
    GetProcessTimes, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ,
    QueryFullProcessImageNameW,
};
use windows::core::PWSTR;

use crate::names::{
    context_switch_counter, counter_process_name, exe_names_match, id_process_counter,
    select_one_pid,
};
use crate::session::{Probe, Reading, SystemClock, sample_to_writer};
use crate::{SampleRequest, SampleRun, SampleTarget, StopReason, ToolError};

const STILL_ACTIVE: u32 = 259;

pub fn run_sample(request: &SampleRequest) -> Result<SampleRun, ToolError> {
    let pid = match &request.target {
        SampleTarget::Pid(pid) => *pid,
        SampleTarget::Name(name) => {
            let pids = find_pids_by_name(name)?;
            select_one_pid(name, &pids)?
        }
    };
    let mut probe = WindowsProbe::open(pid)?;
    let counter = probe.counter.clone();
    let file = std::fs::File::create(&request.out)?;
    let mut writer = io::BufWriter::new(file);
    let mut clock = SystemClock::new();
    let stop = sample_to_writer(
        &mut probe,
        &mut clock,
        request.duration,
        request.interval,
        &mut writer,
    )?;
    writer.flush()?;
    let samples = match stop {
        StopReason::DurationReached { samples } | StopReason::ProcessExited { samples } => samples,
    };
    Ok(SampleRun {
        samples,
        stop,
        pid,
        counter,
        out: request.out.clone(),
    })
}

pub fn find_pids_by_name(query: &str) -> Result<Vec<u32>, ToolError> {
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(|err| ToolError::new(format!("枚举进程失败：{err}")))?;
    let _guard = HandleGuard(snapshot);
    let mut entry = PROCESSENTRY32W {
        dwSize: u32::try_from(std::mem::size_of::<PROCESSENTRY32W>()).unwrap_or(u32::MAX),
        ..PROCESSENTRY32W::default()
    };
    let mut pids = Vec::new();
    let mut has_process = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
    while has_process {
        let exe = wide_from_array(&entry.szExeFile);
        if exe_names_match(query, &exe) {
            pids.push(entry.th32ProcessID);
        }
        has_process = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
    }
    Ok(pids)
}

pub struct WindowsProbe {
    handle: HandleGuard,
    pid: u32,
    counter: String,
    pdh: PdhThreads,
}

impl WindowsProbe {
    pub fn open(pid: u32) -> Result<Self, ToolError> {
        let handle =
            unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) }
                .map_err(|err| ToolError::new(format!("打不开进程 {pid}：{err}")))?;
        let handle = HandleGuard(handle);
        if !process_is_active(handle.0)? {
            return Err(ToolError::new(format!("进程 {pid} 已经退出")));
        }
        let image = process_image(handle.0)?;
        let name = counter_process_name(&image)?;
        let pdh = PdhThreads::open(&name, pid)?;
        Ok(Self {
            handle,
            pid,
            counter: context_switch_counter(&name),
            pdh,
        })
    }
}

impl Probe for WindowsProbe {
    fn read(&mut self) -> Result<Option<Reading>, ToolError> {
        if !process_is_active(self.handle.0)? {
            return Ok(None);
        }
        let mut memory = PROCESS_MEMORY_COUNTERS_EX::default();
        let bytes = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>())
            .map_err(|_| ToolError::new("内存计数结构过大"))?;
        memory.cb = bytes;
        unsafe {
            GetProcessMemoryInfo(
                self.handle.0,
                &mut memory as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS,
                bytes,
            )
        }
        .map_err(|err| ToolError::new(format!("读内存计数失败：{err}")))?;

        let mut handles = 0u32;
        unsafe { GetProcessHandleCount(self.handle.0, &mut handles) }
            .map_err(|err| ToolError::new(format!("读句柄数失败：{err}")))?;

        unsafe { SetLastError(ERROR_SUCCESS) };
        let gdi = unsafe { GetGuiResources(self.handle.0, GR_GDIOBJECTS) };
        unsafe { SetLastError(ERROR_SUCCESS) };
        let user = unsafe { GetGuiResources(self.handle.0, GR_USEROBJECTS) };

        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user_time = FILETIME::default();
        unsafe {
            GetProcessTimes(
                self.handle.0,
                &mut created,
                &mut exited,
                &mut kernel,
                &mut user_time,
            )
        }
        .map_err(|err| ToolError::new(format!("读 CPU 时间失败：{err}")))?;

        Ok(Some(Reading {
            pid: self.pid,
            private_bytes: memory.PrivateUsage as u64,
            working_set_bytes: memory.WorkingSetSize as u64,
            handles: u64::from(handles),
            user_objects: u64::from(user),
            gdi_objects: u64::from(gdi),
            cpu_time_100ns: filetime_u64(kernel).saturating_add(filetime_u64(user_time)),
            wakeups_per_sec: self.pdh.wakeups()?,
        }))
    }
}

struct PdhThreads {
    query: QueryGuard,
    switches: PDH_HCOUNTER,
    ids: PDH_HCOUNTER,
    pid: u32,
}

impl PdhThreads {
    fn open(process_name: &str, pid: u32) -> Result<Self, ToolError> {
        let mut raw = PDH_HQUERY::default();
        pdh(
            unsafe { PdhOpenQueryW(None, 0, &mut raw) },
            "打开性能计数器",
        )?;
        let query = QueryGuard(raw);
        let switches_path = context_switch_counter(process_name);
        let ids_path = id_process_counter(process_name);
        let switches = add_counter_when_ready(query.0, &switches_path)?;
        let ids = add_counter_when_ready(query.0, &ids_path)?;
        let pdh = Self {
            query,
            switches,
            ids,
            pid,
        };
        // 速率计数器要先采集一次，下一次才有每秒值。这一次还没有数据不算失败。
        if let Err(err) = pdh.collect()
            && !err.to_string().contains(INVALID_MARKER)
        {
            return Err(err);
        }
        Ok(pdh)
    }

    fn wakeups(&mut self) -> Result<f64, ToolError> {
        // 新进程要过一会儿才出现在线程计数器里。速率也要两次采集才有效。
        let mut last_error = None;
        for attempt in 0..15 {
            match self.wakeups_once() {
                Ok(value) => return Ok(value),
                Err(err) if err.to_string().contains(INVALID_MARKER) => {
                    last_error = Some(err);
                    if attempt == 14 {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                Err(err) => return Err(err),
            }
        }
        Err(last_error.unwrap_or_else(|| ToolError::new(INVALID_MARKER.to_string())))
    }

    fn wakeups_once(&self) -> Result<f64, ToolError> {
        self.collect()?;
        let switches = counter_doubles(self.switches)?;
        let ids = counter_larges(self.ids)?;
        let mut id_by_name = HashMap::with_capacity(ids.len());
        for (name, pid) in ids {
            id_by_name.insert(name, pid);
        }
        let mut sum = 0.0;
        let mut matched = 0usize;
        for (name, rate) in switches {
            if id_by_name.get(&name).copied() == Some(i64::from(self.pid)) {
                sum += rate;
                matched += 1;
            }
        }
        if matched == 0 {
            return Err(ToolError::new(format!(
                "{INVALID_MARKER}：计数器没有返回 pid {} 的线程",
                self.pid
            )));
        }
        Ok(sum)
    }

    fn collect(&self) -> Result<(), ToolError> {
        let status = unsafe { PdhCollectQueryData(self.query.0) };
        if status_not_ready(status) {
            return Err(ToolError::new(INVALID_MARKER.to_string()));
        }
        pdh(status, "采集性能计数器")
    }
}

struct QueryGuard(PDH_HQUERY);

impl Drop for QueryGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = PdhCloseQuery(self.0);
        }
    }
}

const INVALID_MARKER: &str = "唤醒计数还没有有效数据";

fn add_counter_when_ready(query: PDH_HQUERY, path: &str) -> Result<PDH_HCOUNTER, ToolError> {
    let mut last_error = None;
    for attempt in 0..15 {
        match add_counter(query, path) {
            Ok(counter) => return Ok(counter),
            Err(err) if err.to_string().contains(INVALID_MARKER) => {
                last_error = Some(err);
                if attempt == 14 {
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_error.unwrap_or_else(|| ToolError::new(INVALID_MARKER.to_string())))
}

fn add_counter(query: PDH_HQUERY, path: &str) -> Result<PDH_HCOUNTER, ToolError> {
    let mut wide: Vec<u16> = path.encode_utf16().collect();
    wide.push(0);
    let mut counter = PDH_HCOUNTER::default();
    let status = unsafe { PdhAddEnglishCounterW(query, PWSTR(wide.as_mut_ptr()), 0, &mut counter) };
    if status_not_ready(status) {
        return Err(ToolError::new(format!("{INVALID_MARKER}：{path}")));
    }
    pdh(status, &format!("添加计数器 {path}"))?;
    Ok(counter)
}

fn counter_doubles(counter: PDH_HCOUNTER) -> Result<Vec<(String, f64)>, ToolError> {
    let items = counter_items(counter, PDH_FMT_DOUBLE)?;
    let mut values = Vec::with_capacity(items.len());
    for item in items {
        if !counter_status_ok(item.status) {
            continue;
        }
        values.push((item.name, item.double_value));
    }
    Ok(values)
}

fn counter_larges(counter: PDH_HCOUNTER) -> Result<Vec<(String, i64)>, ToolError> {
    let items = counter_items(counter, PDH_FMT_LARGE)?;
    let mut values = Vec::with_capacity(items.len());
    for item in items {
        if !counter_status_ok(item.status) {
            continue;
        }
        values.push((item.name, item.large_value));
    }
    Ok(values)
}

struct CounterItem {
    name: String,
    status: u32,
    double_value: f64,
    large_value: i64,
}

fn counter_items(counter: PDH_HCOUNTER, format: PDH_FMT) -> Result<Vec<CounterItem>, ToolError> {
    let mut byte_count = 0u32;
    let mut item_count = 0u32;
    let mut buffer: Option<AlignedBuf> = None;
    for _ in 0..6 {
        let status = unsafe {
            PdhGetFormattedCounterArrayW(
                counter,
                format,
                &mut byte_count,
                &mut item_count,
                buffer.as_mut().map(AlignedBuf::as_mut_ptr),
            )
        };
        // 空缓冲区的探测调用会返回 PDH_MORE_DATA，这时 item_count 可能仍是 0。
        if status == PDH_MORE_DATA {
            buffer = Some(AlignedBuf::new(byte_count as usize)?);
            continue;
        }
        if status_not_ready(status) {
            return Err(ToolError::new(INVALID_MARKER.to_string()));
        }
        // 线程退出或实例刚出现时，速率计数器的数组调用返回 PDH_CALC_NEGATIVE_*，
        // 缓冲区里仍有各实例的值。坏实例的 CStatus 下面会丢掉，不能因此停掉整个采样。
        if !formatted_array_has_items(status) {
            return Err(ToolError::new(format!(
                "读取计数器数组失败：0x{status:08X}"
            )));
        }
        if item_count == 0 {
            return Err(ToolError::new(INVALID_MARKER.to_string()));
        }
        let Some(buf) = buffer.as_mut() else {
            return Err(ToolError::new(INVALID_MARKER.to_string()));
        };
        let mut items = Vec::with_capacity(item_count as usize);
        for index in 0..item_count as usize {
            let item = unsafe { &*buf.as_mut_ptr().add(index) };
            let name = wide_ptr_to_string(item.szName);
            let (double_value, large_value) = unsafe {
                (
                    item.FmtValue.Anonymous.doubleValue,
                    item.FmtValue.Anonymous.largeValue,
                )
            };
            items.push(CounterItem {
                name,
                status: item.FmtValue.CStatus,
                double_value,
                large_value,
            });
        }
        return Ok(items);
    }
    Err(ToolError::new(
        "读取计数器数组失败：缓冲区始终不足".to_string(),
    ))
}

/// `PdhGetFormattedCounterArrayW` 在这些状态下仍写出了实例数组。
fn formatted_array_has_items(status: u32) -> bool {
    status == PDH_CSTATUS_VALID_DATA
        || status == PDH_CSTATUS_NEW_DATA
        || status == PDH_CALC_NEGATIVE_VALUE
        || status == PDH_CALC_NEGATIVE_DENOMINATOR
        || status == PDH_CALC_NEGATIVE_TIMEBASE
}

fn status_not_ready(status: u32) -> bool {
    status == PDH_CSTATUS_INVALID_DATA
        || status == PDH_INVALID_DATA
        || status == PDH_NO_DATA
        || status == PDH_CSTATUS_NO_INSTANCE
        || status == PDH_CSTATUS_ITEM_NOT_VALIDATED
}

fn counter_status_ok(status: u32) -> bool {
    status == PDH_CSTATUS_VALID_DATA || status == PDH_CSTATUS_NEW_DATA
}

fn pdh(status: u32, action: &str) -> Result<(), ToolError> {
    if status == PDH_CSTATUS_VALID_DATA || status == PDH_CSTATUS_NEW_DATA {
        Ok(())
    } else {
        Err(ToolError::new(format!("{action}失败：0x{status:08X}")))
    }
}

fn process_is_active(handle: HANDLE) -> Result<bool, ToolError> {
    let mut code = 0u32;
    unsafe { GetExitCodeProcess(handle, &mut code) }
        .map_err(|err| ToolError::new(format!("读取进程退出码失败：{err}")))?;
    Ok(code == STILL_ACTIVE)
}

fn process_image(handle: HANDLE) -> Result<String, ToolError> {
    let mut buffer = vec![0u16; 32_768];
    let mut len = u32::try_from(buffer.len()).unwrap_or(32_768);
    unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
        )
    }
    .map_err(|err| ToolError::new(format!("读取进程路径失败：{err}")))?;
    let text = String::from_utf16_lossy(&buffer[..len as usize]);
    Ok(text)
}

fn filetime_u64(value: FILETIME) -> u64 {
    (u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)
}

fn wide_from_array(values: &[u16]) -> String {
    let end = values
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(values.len());
    String::from_utf16_lossy(&values[..end])
}

fn wide_ptr_to_string(pointer: PWSTR) -> String {
    if pointer.0.is_null() {
        return String::new();
    }
    unsafe {
        let mut len = 0usize;
        while *pointer.0.add(len) != 0 {
            len += 1;
            if len > 4096 {
                break;
            }
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(pointer.0, len))
    }
}

struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct AlignedBuf {
    ptr: *mut PDH_FMT_COUNTERVALUE_ITEM_W,
    layout: std::alloc::Layout,
}

impl AlignedBuf {
    fn new(bytes: usize) -> Result<Self, ToolError> {
        let bytes = bytes.max(std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>());
        let layout = std::alloc::Layout::from_size_align(
            bytes,
            std::mem::align_of::<PDH_FMT_COUNTERVALUE_ITEM_W>(),
        )
        .map_err(|err| ToolError::new(format!("计数器缓冲区对齐失败：{err}")))?;
        let ptr = unsafe { std::alloc::alloc(layout) };
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        Ok(Self {
            ptr: ptr.cast(),
            layout,
        })
    }

    fn as_mut_ptr(&mut self) -> *mut PDH_FMT_COUNTERVALUE_ITEM_W {
        self.ptr
    }
}

impl Drop for AlignedBuf {
    fn drop(&mut self) {
        unsafe { std::alloc::dealloc(self.ptr.cast(), self.layout) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    #[test]
    fn samples_a_short_lived_process_until_it_exits() {
        let mut child = Command::new("ping")
            .args(["-4", "-n", "12", "127.0.0.1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn ping");
        let dir = std::env::temp_dir().join(format!("lanwork-sample-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("ping.csv");
        let result = run_sample(&SampleRequest {
            target: SampleTarget::Pid(child.id()),
            duration: Duration::from_secs(30),
            interval: Duration::from_secs(1),
            out: out.clone(),
            user_profile: None,
        });
        let _ = child.kill();
        let _ = child.wait();
        let run = result.expect("sample ping");
        assert!(run.samples >= 1, "samples={}", run.samples);
        assert!(matches!(run.stop, StopReason::ProcessExited { .. }));
        assert!(run.counter.contains("Context Switches/sec"));
        let csv = std::fs::read_to_string(&out).unwrap();
        let mut lines = csv.lines();
        let header = lines.next().unwrap();
        assert!(header.contains("private_bytes"));
        assert!(header.contains("wakeups_per_sec"));
        let row = lines.next().expect("at least one sample row");
        let fields: Vec<_> = row.split(',').collect();
        assert_eq!(fields.len(), 10);
        let private_bytes: u64 = fields[2].parse().unwrap();
        assert!(private_bytes > 0);
        let wakeups: f64 = fields[9].parse().unwrap();
        assert!(wakeups.is_finite() && wakeups >= 0.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn negative_rate_status_still_returns_the_counter_array() {
        assert!(formatted_array_has_items(PDH_CSTATUS_VALID_DATA));
        assert!(formatted_array_has_items(PDH_CSTATUS_NEW_DATA));
        assert!(formatted_array_has_items(PDH_CALC_NEGATIVE_VALUE));
        assert!(formatted_array_has_items(PDH_CALC_NEGATIVE_DENOMINATOR));
        assert!(formatted_array_has_items(PDH_CALC_NEGATIVE_TIMEBASE));
        assert!(!formatted_array_has_items(PDH_MORE_DATA));
        assert!(!formatted_array_has_items(0x8000_07D0));
        assert!(!counter_status_ok(PDH_CALC_NEGATIVE_VALUE));
    }

    #[test]
    fn finds_the_spawned_process_by_name() {
        let mut child = Command::new("ping")
            .args(["-n", "6", "127.0.0.1"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn ping");
        let pids = find_pids_by_name("ping").unwrap();
        assert!(pids.contains(&child.id()));
        let _ = child.kill();
        let _ = child.wait();
    }
}
