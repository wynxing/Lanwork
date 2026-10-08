//! 用 LoadLibraryW 加载随包的 Everything SDK DLL。
//!
//! 先探测 SDK3（1.5），再探测 SDK（1.4）。1.4 的 DLL 只连接未命名实例。

use std::ffi::c_void;
use std::mem::{self, size_of};
use std::path::{Path, PathBuf};

use serde::Serialize;
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::s;

use crate::host::{FileIdxError, mono_ns, pcwstr, string_from_wide_buf, wide_null, wide_path};
use crate::state::{
    EVERYTHING3_ERROR_IPC_PIPE_NOT_FOUND, MachineState, ProbeKind, clamp_limit, classify_sdk3,
    classify_sdk14, machine_state,
};

const ALPHA_INSTANCE: &str = "1.5a";
const NAME_BUF: usize = 32_768;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdkChoice {
    Auto,
    Sdk3,
    Sdk14,
}

#[derive(Debug, Clone, Serialize)]
pub struct InstallSnapshot {
    pub registry_hklm: bool,
    pub registry_hkcu: bool,
    pub service: Option<String>,
    pub program_files_exe: Vec<String>,
    pub system_installed: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SdkAttempt {
    pub sdk: String,
    pub instance: Option<String>,
    pub dll_path: String,
    pub connect_ok: bool,
    pub is_db_loaded: Option<bool>,
    pub last_error: Option<u32>,
    pub last_error_hex: Option<String>,
    pub kind: ProbeKind,
    pub server_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeReport {
    pub install: InstallSnapshot,
    pub client_present: bool,
    pub attempts: Vec<SdkAttempt>,
    pub selected: Option<String>,
    pub kind: ProbeKind,
    pub state: MachineState,
}

#[derive(Debug, Clone, Serialize)]
pub struct EverythingQuery {
    pub sdk: String,
    pub instance: Option<String>,
    pub kind: ProbeKind,
    pub state: MachineState,
    pub elapsed_ns: Option<u64>,
    pub returned: usize,
    pub total: Option<u64>,
    pub limit: usize,
    pub expect_hit: Option<bool>,
    pub reject_hit: Option<bool>,
    pub names: Vec<String>,
    pub matching_names: Vec<String>,
    pub last_error: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CapCheck {
    pub sdk: String,
    pub at_50: usize,
    pub at_51: usize,
    pub total: Option<u64>,
    pub cap_holds: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PollSample {
    pub t_ms: u64,
    pub is_db_loaded: bool,
    pub last_error: u32,
    pub kind: ProbeKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct PollReport {
    pub sdk: String,
    pub instance: Option<String>,
    pub samples: Vec<PollSample>,
    pub saw_not_ready: bool,
    pub saw_ready: bool,
    pub saw_not_running: bool,
}

pub fn dll_path(sdk: SdkChoice) -> PathBuf {
    let (dir, file) = match sdk {
        SdkChoice::Sdk14 => ("sdk", "Everything64.dll"),
        SdkChoice::Sdk3 | SdkChoice::Auto => ("sdk3", "Everything3_x64.dll"),
    };
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("third_party")
        .join("everything")
        .join(dir)
        .join(file)
}

pub fn install_snapshot() -> InstallSnapshot {
    let registry_hklm = reg_key_exists(true, "SOFTWARE\\voidtools\\Everything");
    let registry_hkcu = reg_key_exists(false, "SOFTWARE\\voidtools\\Everything");
    let service = everything_service_state();
    let mut program_files_exe = Vec::new();
    for path in [
        r"C:\Program Files\Everything\Everything.exe",
        r"C:\Program Files (x86)\Everything\Everything.exe",
    ] {
        if Path::new(path).is_file() {
            program_files_exe.push(path.to_string());
        }
    }
    let service_installed = service.is_some();
    let system_installed =
        registry_hklm || registry_hkcu || service_installed || !program_files_exe.is_empty();
    InstallSnapshot {
        registry_hklm,
        registry_hkcu,
        service,
        program_files_exe,
        system_installed,
    }
}

pub fn probe(choice: SdkChoice, client_present: Option<bool>) -> Result<ProbeReport, FileIdxError> {
    let install = install_snapshot();
    let present = client_present.unwrap_or(install.system_installed);
    let mut attempts = Vec::new();
    match choice {
        SdkChoice::Auto => {
            attempts.extend(probe_sdk3_instances()?);
            let sdk3_connected = attempts.iter().any(|item| item.connect_ok);
            if !sdk3_connected {
                attempts.push(probe_sdk14()?);
            }
        }
        SdkChoice::Sdk3 => attempts.extend(probe_sdk3_instances()?),
        SdkChoice::Sdk14 => attempts.push(probe_sdk14()?),
    }
    let selected = attempts.iter().find(|item| item.connect_ok);
    let kind = selected
        .map(|item| item.kind)
        .unwrap_or(ProbeKind::NotRunning);
    let selected_name = selected.map(|item| match &item.instance {
        Some(name) if !name.is_empty() => format!("{}:{}", item.sdk, name),
        _ => item.sdk.clone(),
    });
    Ok(ProbeReport {
        install,
        client_present: present,
        state: machine_state(present, kind),
        kind,
        selected: selected_name,
        attempts,
    })
}

pub fn probe_one(
    choice: SdkChoice,
    instance: Option<&str>,
    client_present: Option<bool>,
) -> Result<ProbeReport, FileIdxError> {
    let install = install_snapshot();
    let present = client_present.unwrap_or(install.system_installed);
    let attempt = match choice {
        SdkChoice::Sdk14 => probe_sdk14()?,
        SdkChoice::Sdk3 | SdkChoice::Auto => {
            let mut lib = Sdk3::load()?;
            lib.probe_instance(instance)
        }
    };
    let kind = attempt.kind;
    Ok(ProbeReport {
        install,
        client_present: present,
        selected: attempt.connect_ok.then(|| attempt.sdk.clone()),
        state: machine_state(present, kind),
        kind,
        attempts: vec![attempt],
    })
}

pub fn query_everything(
    choice: SdkChoice,
    instance: Option<&str>,
    text: &str,
    limit: usize,
    expect: Option<&str>,
    reject: Option<&str>,
    client_present: Option<bool>,
) -> Result<EverythingQuery, FileIdxError> {
    let install = install_snapshot();
    let present = client_present.unwrap_or(install.system_installed);
    let limit = clamp_limit(limit);
    match choice {
        SdkChoice::Sdk14 => {
            let mut lib = Sdk14::load()?;
            lib.query(text, limit, expect, reject, present)
        }
        SdkChoice::Sdk3 => {
            let mut lib = Sdk3::load()?;
            lib.query(instance, text, limit, expect, reject, present)
        }
        SdkChoice::Auto => {
            let mut sdk3 = Sdk3::load()?;
            let first = sdk3.probe_instance(None);
            if first.connect_ok {
                return sdk3.query(None, text, limit, expect, reject, present);
            }
            if instance.is_none() {
                let alpha = sdk3.probe_instance(Some(ALPHA_INSTANCE));
                if alpha.connect_ok {
                    return sdk3.query(Some(ALPHA_INSTANCE), text, limit, expect, reject, present);
                }
            }
            let mut sdk14 = Sdk14::load()?;
            sdk14.query(text, limit, expect, reject, present)
        }
    }
}

pub fn cap_check(choice: SdkChoice, instance: Option<&str>) -> Result<CapCheck, FileIdxError> {
    match choice {
        SdkChoice::Sdk14 => Sdk14::load()?.cap_check(),
        SdkChoice::Sdk3 | SdkChoice::Auto => Sdk3::load()?.cap_check(instance),
    }
}

pub fn poll(
    choice: SdkChoice,
    instance: Option<&str>,
    millis: u64,
) -> Result<PollReport, FileIdxError> {
    let started = std::time::Instant::now();
    let mut samples = Vec::new();
    let mut last_key: Option<(bool, u32, ProbeKind)> = None;
    let sdk_name;
    let used_instance;
    match choice {
        SdkChoice::Sdk14 => {
            let lib = Sdk14::load()?;
            sdk_name = "sdk14".to_string();
            used_instance = None;
            while started.elapsed().as_millis() < u128::from(millis) {
                let (loaded, err) = lib.db_status();
                let kind = classify_sdk14(loaded, err);
                push_poll(
                    &mut samples,
                    &mut last_key,
                    started.elapsed().as_millis() as u64,
                    loaded,
                    err,
                    kind,
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
        SdkChoice::Sdk3 | SdkChoice::Auto => {
            let mut lib = Sdk3::load()?;
            let inst = instance;
            sdk_name = "sdk3".to_string();
            used_instance = inst.map(str::to_string);
            while started.elapsed().as_millis() < u128::from(millis) {
                let attempt = lib.probe_instance(inst);
                let loaded = attempt.is_db_loaded.unwrap_or(false);
                let err = attempt.last_error.unwrap_or(0);
                push_poll(
                    &mut samples,
                    &mut last_key,
                    started.elapsed().as_millis() as u64,
                    loaded,
                    err,
                    attempt.kind,
                );
                if attempt.connect_ok {
                    lib.drop_client();
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }
    }
    Ok(PollReport {
        sdk: sdk_name,
        instance: used_instance,
        saw_not_ready: samples.iter().any(|s| s.kind == ProbeKind::NotReady),
        saw_ready: samples.iter().any(|s| s.kind == ProbeKind::Ready),
        saw_not_running: samples.iter().any(|s| s.kind == ProbeKind::NotRunning),
        samples,
    })
}

fn push_poll(
    samples: &mut Vec<PollSample>,
    last_key: &mut Option<(bool, u32, ProbeKind)>,
    t_ms: u64,
    is_db_loaded: bool,
    last_error: u32,
    kind: ProbeKind,
) {
    let key = (is_db_loaded, last_error, kind);
    if samples.is_empty() || last_key.is_some_and(|prev| prev != key) {
        if samples.len() < 200 {
            samples.push(PollSample {
                t_ms,
                is_db_loaded,
                last_error,
                kind,
            });
        }
        *last_key = Some(key);
    }
}

struct Sdk14 {
    module: HMODULE,
    set_search: SetSearchW,
    set_bool: SetBool,
    set_dword: SetDword,
    query: QueryW,
    last_error: GetDword,
    is_db_loaded: GetBool,
    num_results: GetDword,
    tot_results: GetDword,
    is_folder: IsFolder14,
    full_path: FullPath14,
    file_name: Name14,
}

type SetSearchW = unsafe extern "system" fn(*const u16);
type SetBool = unsafe extern "system" fn(i32);
type SetDword = unsafe extern "system" fn(u32);
type QueryW = unsafe extern "system" fn(i32) -> i32;
type GetDword = unsafe extern "system" fn() -> u32;
type GetBool = unsafe extern "system" fn() -> i32;
type IsFolder14 = unsafe extern "system" fn(u32) -> i32;
type FullPath14 = unsafe extern "system" fn(u32, *mut u16, u32) -> u32;
type Name14 = unsafe extern "system" fn(u32) -> *const u16;

impl Sdk14 {
    fn load() -> Result<Self, FileIdxError> {
        let module = load_dll(&dll_path(SdkChoice::Sdk14))?;
        Ok(Self {
            module,
            set_search: sym(module, s!("Everything_SetSearchW"))?,
            set_bool: sym(module, s!("Everything_SetMatchPath"))?,
            set_dword: sym(module, s!("Everything_SetMax"))?,
            query: sym(module, s!("Everything_QueryW"))?,
            last_error: sym(module, s!("Everything_GetLastError"))?,
            is_db_loaded: sym(module, s!("Everything_IsDBLoaded"))?,
            num_results: sym(module, s!("Everything_GetNumResults"))?,
            tot_results: sym(module, s!("Everything_GetTotResults"))?,
            is_folder: sym(module, s!("Everything_IsFolderResult"))?,
            full_path: sym(module, s!("Everything_GetResultFullPathNameW"))?,
            file_name: sym(module, s!("Everything_GetResultFileNameW"))?,
        })
    }

    fn db_status(&self) -> (bool, u32) {
        // SAFETY: DLL 已加载，函数指针来自该 DLL。调用后立刻读 last error。
        let loaded = unsafe { (self.is_db_loaded)() } != 0;
        let err = unsafe { (self.last_error)() };
        (loaded, err)
    }

    fn status_attempt(&self) -> SdkAttempt {
        let (loaded, err) = self.db_status();
        attempt(
            "sdk14",
            None,
            &dll_path(SdkChoice::Sdk14),
            !(!loaded && err == crate::state::EVERYTHING_ERROR_IPC),
            Some(loaded),
            err,
            classify_sdk14(loaded, err),
            None,
        )
    }

    fn query(
        &mut self,
        text: &str,
        limit: usize,
        expect: Option<&str>,
        reject: Option<&str>,
        client_present: bool,
    ) -> Result<EverythingQuery, FileIdxError> {
        let (loaded, err) = self.db_status();
        let kind = classify_sdk14(loaded, err);
        if kind != ProbeKind::Ready {
            return Ok(empty_query("sdk14", None, kind, client_present, err, limit));
        }
        let wide = wide_null(text);
        let start = mono_ns();
        // SAFETY: 搜索串以 0 结尾。SetMax 不超过 50。QueryW 的 TRUE 表示阻塞等待。
        let ok = unsafe {
            (self.set_search)(wide.as_ptr());
            (self.set_bool)(0);
            (self.set_dword)(u32::try_from(limit).unwrap_or(50));
            (self.query)(1) != 0
        };
        let err = unsafe { (self.last_error)() };
        if !ok {
            let elapsed = mono_ns().saturating_sub(start);
            let kind = classify_sdk14(false, err);
            let mut report = empty_query("sdk14", None, kind, client_present, err, limit);
            report.elapsed_ns = Some(elapsed);
            return Ok(report);
        }
        let returned_raw = unsafe { (self.num_results)() };
        let total = unsafe { (self.tot_results)() };
        let take = usize::try_from(returned_raw).unwrap_or(0).min(limit);
        let mut names = Vec::new();
        for index in 0..take {
            let index = u32::try_from(index).unwrap_or(u32::MAX);
            let ptr = unsafe { (self.file_name)(index) };
            names.push(string_from_wide_buf(unsafe { wide_slice(ptr) }));
            let mut path_buf = vec![0u16; NAME_BUF];
            unsafe {
                let _ = (self.is_folder)(index);
                let _ = (self.full_path)(
                    index,
                    path_buf.as_mut_ptr(),
                    u32::try_from(path_buf.len()).unwrap_or(0),
                );
            }
        }
        let elapsed = mono_ns().saturating_sub(start);
        Ok(finish_query(
            "sdk14",
            None,
            client_present,
            elapsed,
            names,
            Some(u64::from(total)),
            limit,
            expect,
            reject,
            None,
        ))
    }

    fn cap_check(&mut self) -> Result<CapCheck, FileIdxError> {
        let at_50 = self.count_for_max(50)?;
        let at_51 = self.count_for_max(51)?;
        Ok(CapCheck {
            sdk: "sdk14".to_string(),
            at_50: at_50.0,
            at_51: at_51.0,
            total: Some(u64::from(at_51.1.max(at_50.1))),
            cap_holds: at_50.0 <= 50 && (at_51.1 <= 50 || at_51.0 > at_50.0 || at_50.0 == 50),
        })
    }

    fn count_for_max(&mut self, max: u32) -> Result<(usize, u32), FileIdxError> {
        let (loaded, err) = self.db_status();
        if classify_sdk14(loaded, err) != ProbeKind::Ready {
            return Err(FileIdxError::new(format!(
                "Everything 1.4 未就绪，IsDBLoaded={loaded} LastError={err}"
            )));
        }
        let star = wide_null("*");
        // SAFETY: 查询 `*`，只取数量，不保留路径。
        let ok = unsafe {
            (self.set_search)(star.as_ptr());
            (self.set_bool)(0);
            (self.set_dword)(max);
            (self.query)(1) != 0
        };
        if !ok {
            let err = unsafe { (self.last_error)() };
            return Err(FileIdxError::new(format!(
                "Everything 1.4 查询失败，LastError={err}"
            )));
        }
        let num = unsafe { (self.num_results)() };
        let tot = unsafe { (self.tot_results)() };
        Ok((usize::try_from(num).unwrap_or(0), tot))
    }
}

impl Drop for Sdk14 {
    fn drop(&mut self) {
        // SAFETY: 这个模块只由本结构持有。
        unsafe {
            let _ = FreeLibrary(self.module);
        }
    }
}

struct Sdk3 {
    module: HMODULE,
    connect: ConnectW,
    destroy: DestroyClient,
    last_error: GetDword,
    is_db_loaded: IsDb3,
    major: Ver3,
    minor: Ver3,
    revision: Ver3,
    build: Ver3,
    create_state: CreateState,
    destroy_state: DestroyState,
    set_text: SetText,
    set_viewport: SetSize,
    add_property: AddProperty,
    set_match_path: SetStateBool,
    search: SearchFn,
    destroy_results: DestroyResults,
    count: CountFn,
    name: Name3,
    is_folder: IsFolder3,
    client: *mut c_void,
}

type ConnectW = unsafe extern "system" fn(*const u16) -> *mut c_void;
type DestroyClient = unsafe extern "system" fn(*mut c_void) -> i32;
type IsDb3 = unsafe extern "system" fn(*mut c_void) -> i32;
type Ver3 = unsafe extern "system" fn(*mut c_void) -> u32;
type CreateState = unsafe extern "system" fn() -> *mut c_void;
type DestroyState = unsafe extern "system" fn(*mut c_void) -> i32;
type SetText = unsafe extern "system" fn(*mut c_void, *const u16) -> i32;
type SetSize = unsafe extern "system" fn(*mut c_void, usize) -> i32;
type AddProperty = unsafe extern "system" fn(*mut c_void, u32) -> i32;
type SetStateBool = unsafe extern "system" fn(*mut c_void, i32) -> i32;
type SearchFn = unsafe extern "system" fn(*mut c_void, *mut c_void) -> *mut c_void;
type DestroyResults = unsafe extern "system" fn(*mut c_void) -> i32;
type CountFn = unsafe extern "system" fn(*mut c_void) -> usize;
type Name3 = unsafe extern "system" fn(*mut c_void, usize, *mut u16, usize) -> usize;
type IsFolder3 = unsafe extern "system" fn(*mut c_void, usize) -> i32;

impl Sdk3 {
    fn load() -> Result<Self, FileIdxError> {
        let module = load_dll(&dll_path(SdkChoice::Sdk3))?;
        Ok(Self {
            module,
            connect: sym(module, s!("Everything3_ConnectW"))?,
            destroy: sym(module, s!("Everything3_DestroyClient"))?,
            last_error: sym(module, s!("Everything3_GetLastError"))?,
            is_db_loaded: sym(module, s!("Everything3_IsDBLoaded"))?,
            major: sym(module, s!("Everything3_GetMajorVersion"))?,
            minor: sym(module, s!("Everything3_GetMinorVersion"))?,
            revision: sym(module, s!("Everything3_GetRevision"))?,
            build: sym(module, s!("Everything3_GetBuildNumber"))?,
            create_state: sym(module, s!("Everything3_CreateSearchState"))?,
            destroy_state: sym(module, s!("Everything3_DestroySearchState"))?,
            set_text: sym(module, s!("Everything3_SetSearchTextW"))?,
            set_viewport: sym(module, s!("Everything3_SetSearchViewportCount"))?,
            add_property: sym(module, s!("Everything3_AddSearchPropertyRequest"))?,
            set_match_path: sym(module, s!("Everything3_SetSearchMatchPath"))?,
            search: sym(module, s!("Everything3_Search"))?,
            destroy_results: sym(module, s!("Everything3_DestroyResultList"))?,
            count: sym(module, s!("Everything3_GetResultListCount"))?,
            name: sym(module, s!("Everything3_GetResultNameW"))?,
            is_folder: sym(module, s!("Everything3_IsFolderResult"))?,
            client: std::ptr::null_mut(),
        })
    }

    fn drop_client(&mut self) {
        if !self.client.is_null() {
            // SAFETY: client 来自 ConnectW，只释放一次。
            unsafe {
                (self.destroy)(self.client);
            }
            self.client = std::ptr::null_mut();
        }
    }

    fn connect_instance(&mut self, instance: Option<&str>) -> (bool, u32) {
        self.drop_client();
        let wide;
        let ptr = if let Some(name) = instance {
            if name.is_empty() {
                std::ptr::null()
            } else {
                wide = wide_null(name);
                wide.as_ptr()
            }
        } else {
            std::ptr::null()
        };
        // SAFETY: 实例名要么是空指针，要么以 0 结尾。失败后立刻读 GetLastError。
        let client = unsafe { (self.connect)(ptr) };
        let err = unsafe { (self.last_error)() };
        if client.is_null() {
            (
                false,
                if err == 0 {
                    EVERYTHING3_ERROR_IPC_PIPE_NOT_FOUND
                } else {
                    err
                },
            )
        } else {
            self.client = client;
            (true, err)
        }
    }

    fn probe_instance(&mut self, instance: Option<&str>) -> SdkAttempt {
        let (connected, err) = self.connect_instance(instance);
        if !connected {
            return attempt(
                "sdk3",
                instance.map(str::to_string),
                &dll_path(SdkChoice::Sdk3),
                false,
                None,
                err,
                classify_sdk3(false, false, err),
                None,
            );
        }
        // SAFETY: client 刚连接成功。
        let loaded = unsafe { (self.is_db_loaded)(self.client) } != 0;
        let err = unsafe { (self.last_error)() };
        let version = if loaded {
            let major = unsafe { (self.major)(self.client) };
            let minor = unsafe { (self.minor)(self.client) };
            let revision = unsafe { (self.revision)(self.client) };
            let build = unsafe { (self.build)(self.client) };
            Some(format!("{major}.{minor}.{revision}.{build}"))
        } else {
            None
        };
        attempt(
            "sdk3",
            instance.map(str::to_string),
            &dll_path(SdkChoice::Sdk3),
            true,
            Some(loaded),
            err,
            classify_sdk3(true, loaded, err),
            version,
        )
    }

    fn query(
        &mut self,
        instance: Option<&str>,
        text: &str,
        limit: usize,
        expect: Option<&str>,
        reject: Option<&str>,
        client_present: bool,
    ) -> Result<EverythingQuery, FileIdxError> {
        let status = self.probe_instance(instance);
        if status.kind != ProbeKind::Ready {
            return Ok(empty_query(
                "sdk3",
                instance.map(str::to_string),
                status.kind,
                client_present,
                status.last_error.unwrap_or(0),
                limit,
            ));
        }
        let names = self.search_names(text, limit)?;
        let elapsed = names.1;
        Ok(finish_query(
            "sdk3",
            instance.map(str::to_string),
            client_present,
            elapsed,
            names.0,
            None,
            limit,
            expect,
            reject,
            None,
        ))
    }

    fn search_names(
        &mut self,
        text: &str,
        limit: usize,
    ) -> Result<(Vec<String>, u64), FileIdxError> {
        let wide = wide_null(text);
        let start = mono_ns();
        // SAFETY: client 已连接。search state 在本函数结束前销毁。
        let state = unsafe { (self.create_state)() };
        if state.is_null() {
            let err = unsafe { (self.last_error)() };
            return Err(FileIdxError::new(format!(
                "CreateSearchState 失败，LastError={err:#X}"
            )));
        }
        let results = unsafe {
            (self.set_text)(state, wide.as_ptr());
            (self.set_match_path)(state, 0);
            (self.set_viewport)(state, limit);
            // 不请求 NAME 时，GetResultNameW 得到空串。0 是 EVERYTHING3_PROPERTY_ID_NAME。
            (self.add_property)(state, 0);
            (self.search)(self.client, state)
        };
        let err = unsafe { (self.last_error)() };
        if results.is_null() {
            unsafe {
                (self.destroy_state)(state);
            }
            return Err(FileIdxError::new(format!(
                "Everything3_Search 失败，LastError={err:#X}"
            )));
        }
        let count = unsafe { (self.count)(results) }.min(limit);
        let mut names = Vec::with_capacity(count);
        for index in 0..count {
            let mut buf = vec![0u16; NAME_BUF];
            unsafe {
                (self.name)(results, index, buf.as_mut_ptr(), buf.len());
                let _ = (self.is_folder)(results, index);
            }
            names.push(string_from_wide_buf(&buf));
        }
        unsafe {
            (self.destroy_results)(results);
            (self.destroy_state)(state);
        }
        Ok((names, mono_ns().saturating_sub(start)))
    }

    fn cap_check(&mut self, instance: Option<&str>) -> Result<CapCheck, FileIdxError> {
        let status = self.probe_instance(instance);
        if status.kind != ProbeKind::Ready {
            return Err(FileIdxError::new(format!(
                "Everything 1.5 未就绪：{:?}",
                status.kind
            )));
        }
        let at_50 = self.search_names("*", 50)?.0.len();
        let at_51 = self.search_names("*", 51)?.0.len();
        Ok(CapCheck {
            sdk: "sdk3".to_string(),
            at_50,
            at_51,
            total: None,
            cap_holds: at_50 <= 50 && (at_51 > 50 || at_50 < 51),
        })
    }
}

impl Drop for Sdk3 {
    fn drop(&mut self) {
        self.drop_client();
        unsafe {
            let _ = FreeLibrary(self.module);
        }
    }
}

fn probe_sdk3_instances() -> Result<Vec<SdkAttempt>, FileIdxError> {
    let mut lib = Sdk3::load()?;
    let unnamed = lib.probe_instance(None);
    if unnamed.connect_ok {
        return Ok(vec![unnamed]);
    }
    let alpha = lib.probe_instance(Some(ALPHA_INSTANCE));
    Ok(vec![unnamed, alpha])
}

fn probe_sdk14() -> Result<SdkAttempt, FileIdxError> {
    Ok(Sdk14::load()?.status_attempt())
}

fn load_dll(path: &Path) -> Result<HMODULE, FileIdxError> {
    let path = path
        .canonicalize()
        .map_err(|err| FileIdxError::new(format!("找不到 {}：{err}", path.display())))?;
    let wide = wide_path(&path);
    // SAFETY: 路径以 0 结尾，指向我们刚分配的缓冲。
    unsafe { LoadLibraryW(pcwstr(&wide)) }
        .map_err(|err| FileIdxError::new(format!("LoadLibraryW {} 失败：{err}", path.display())))
}

fn sym<T>(module: HMODULE, name: windows::core::PCSTR) -> Result<T, FileIdxError> {
    // SAFETY: 模块句柄仍然有效。只在函数指针非空时做同样大小的拷贝。
    let proc = unsafe { GetProcAddress(module, name) }.ok_or_else(|| {
        FileIdxError::new(format!(
            "DLL 里没有 {}",
            unsafe { name.to_string() }.unwrap_or_default()
        ))
    })?;
    if size_of::<T>() != size_of_val(&proc) {
        return Err(FileIdxError::new("函数指针大小不一致"));
    }
    Ok(unsafe { mem::transmute_copy(&proc) })
}

unsafe fn wide_slice<'a>(ptr: *const u16) -> &'a [u16] {
    if ptr.is_null() {
        return &[];
    }
    let mut len = 0usize;
    while unsafe { *ptr.add(len) } != 0 && len < NAME_BUF {
        len += 1;
    }
    unsafe { std::slice::from_raw_parts(ptr, len) }
}

#[allow(clippy::too_many_arguments)]
fn attempt(
    sdk: &str,
    instance: Option<String>,
    path: &Path,
    connect_ok: bool,
    is_db_loaded: Option<bool>,
    last_error: u32,
    kind: ProbeKind,
    server_version: Option<String>,
) -> SdkAttempt {
    SdkAttempt {
        sdk: sdk.to_string(),
        instance,
        dll_path: path.display().to_string(),
        connect_ok,
        is_db_loaded,
        last_error: Some(last_error),
        last_error_hex: Some(format!("{last_error:#X}")),
        kind,
        server_version,
    }
}

fn empty_query(
    sdk: &str,
    instance: Option<String>,
    kind: ProbeKind,
    client_present: bool,
    last_error: u32,
    limit: usize,
) -> EverythingQuery {
    let _ = machine_state(client_present, kind);
    EverythingQuery {
        sdk: sdk.to_string(),
        instance,
        kind,
        state: machine_state(client_present, kind),
        elapsed_ns: None,
        returned: 0,
        total: None,
        limit,
        expect_hit: None,
        reject_hit: None,
        names: Vec::new(),
        matching_names: Vec::new(),
        last_error: Some(last_error),
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_query(
    sdk: &str,
    instance: Option<String>,
    client_present: bool,
    elapsed: u64,
    names: Vec<String>,
    total: Option<u64>,
    limit: usize,
    expect: Option<&str>,
    reject: Option<&str>,
    last_error: Option<u32>,
) -> EverythingQuery {
    let expect_hit = expect.map(|needle| names.iter().any(|name| name_eq(name, needle)));
    let reject_hit = reject.map(|needle| names.iter().any(|name| name_eq(name, needle)));
    let mut matching_names = Vec::new();
    for name in &names {
        if expect.is_some_and(|needle| name_eq(name, needle))
            || reject.is_some_and(|needle| name_eq(name, needle))
        {
            matching_names.push(name.clone());
        }
    }
    EverythingQuery {
        sdk: sdk.to_string(),
        instance,
        kind: ProbeKind::Ready,
        state: machine_state(client_present, ProbeKind::Ready),
        elapsed_ns: Some(elapsed),
        returned: names.len(),
        total,
        limit,
        expect_hit,
        reject_hit,
        names,
        matching_names,
        last_error,
    }
}

fn name_eq(left: &str, right: &str) -> bool {
    if left.eq_ignore_ascii_case(right) {
        return true;
    }
    std::path::Path::new(left)
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case(right))
}

fn reg_key_exists(hklm: bool, subkey: &str) -> bool {
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY, RegCloseKey,
        RegOpenKeyExW,
    };
    let root = if hklm {
        HKEY_LOCAL_MACHINE
    } else {
        HKEY_CURRENT_USER
    };
    let wide = wide_null(subkey);
    let mut key = HKEY_CURRENT_USER;
    // SAFETY: 子键以 0 结尾。打开成功才关闭。
    let status = unsafe {
        RegOpenKeyExW(
            root,
            pcwstr(&wide),
            Some(0),
            KEY_READ | KEY_WOW64_64KEY,
            &mut key,
        )
    };
    if status == ERROR_SUCCESS {
        unsafe {
            let _ = RegCloseKey(key);
        }
        true
    } else {
        false
    }
}

fn everything_service_state() -> Option<String> {
    use windows::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
        SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS,
    };
    // SAFETY: 只查询服务状态，不启动也不停止。
    let scm = unsafe { OpenSCManagerW(None, None, SC_MANAGER_CONNECT) }.ok()?;
    let service =
        unsafe { OpenServiceW(scm, windows::core::w!("Everything"), SERVICE_QUERY_STATUS) };
    let Ok(service) = service else {
        unsafe {
            let _ = CloseServiceHandle(scm);
        }
        return None;
    };
    let mut status = SERVICE_STATUS::default();
    let queried = unsafe { QueryServiceStatus(service, &mut status) };
    unsafe {
        let _ = CloseServiceHandle(service);
        let _ = CloseServiceHandle(scm);
    }
    queried.ok()?;
    let state = if status.dwCurrentState == SERVICE_RUNNING {
        "running"
    } else {
        "installed-not-running"
    };
    Some(state.to_string())
}
