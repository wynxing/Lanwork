use std::process::{Child, Command};
use std::time::Duration;

use crate::activator::{self, activate_in_process, call_activate, guid_from_hex, hex_clsid};
use crate::model::{ACTIVATOR_CLSID_U128, AUMID};
use crate::notify::set_process_aumid;
use crate::panel;
use crate::util::{ComApartment, SpikeError, SpikeResult, exe_path, log_line};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{GetStartupInfoW, STARTUPINFOW};
use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

pub fn activation_check() -> SpikeResult<()> {
    log_line("activation-check 开始。不写 HKCU，不代替人工点击通知。");
    log_process("activation-check");
    let _com = ComApartment::new()?;
    set_process_aumid()?;
    crate::set_dpi();
    let others = other_toast_pids()?;
    let (guid, value, label) = if others.is_empty() {
        log_line("没有其他 toast.exe，向 SCM 注册产品 CLSID");
        (
            crate::registry::activator_guid(),
            ACTIVATOR_CLSID_U128,
            "产品 CLSID",
        )
    } else {
        log_line(&format!(
            "已有其他 toast.exe pid={others:?}，不注册产品 CLSID，避免抢走正在运行的服务器。改用仅内存中的探测 CLSID"
        ));
        (
            activator::probe_guid(),
            activator::PROBE_CLSID_U128,
            "探测 CLSID",
        )
    };

    let _guard = ActivationGuard::arm();
    activator::register_class_guid(guid, value)?;
    let hwnd = panel::startup()?;
    if !panel::is_main_visible() {
        return Err(SpikeError::new(
            "模拟面板 force_show 之后 IsWindowVisible 仍为 false",
        ));
    }
    log_line(&format!(
        "面板已显示 hwnd={}，开始进程内激活",
        hwnd.0 as isize
    ));

    activate_in_process("todo-001")?;
    if !panel::pump_until(Duration::from_secs(2), located("todo-001"))? {
        return Err(SpikeError::new(
            "进程内 Activate 后 2 秒内没有定位到 todo-001",
        ));
    }
    expect_located("todo-001")?;
    println!("activation-check: 进程内已定位 todo-001，窗口可见");

    activator::resume_class()?;
    let mut child = KillChild(spawn_client(value, "todo-003")?);
    let arrived = panel::pump_until(Duration::from_secs(8), located("todo-003"))?;
    let status = wait_child(&mut child.0, Duration::from_secs(3))?;
    if !arrived {
        return Err(SpikeError::new(format!(
            "跨进程 CoCreateInstance + Activate 后 8 秒内没有定位到 todo-003（{label} {}，子进程退出码 {status}）",
            hex_clsid(value)
        )));
    }
    if !status.success() {
        return Err(SpikeError::new(format!(
            "activation-client 退出码 {status}"
        )));
    }
    expect_located("todo-003")?;
    println!(
        "activation-check: 跨进程已定位 todo-003，窗口可见（{label} {}）",
        hex_clsid(value)
    );
    log_line("activation-check 结束");
    Ok(())
}

pub fn activation_client(args: &[String]) -> SpikeResult<()> {
    let (hex, launch, server_pid) = match args {
        [hex, launch, pid] => (hex.as_str(), launch.as_str(), pid.as_str()),
        _ => {
            return Err(SpikeError::new(
                "用法：activation-client <clsid 十六进制> <launch> <服务器 pid>",
            ));
        }
    };
    let server_pid = server_pid
        .parse::<u32>()
        .map_err(|_| SpikeError::new("服务器 pid 不是数字"))?;
    log_line(&format!(
        "activation-client 启动 clsid={hex} launch={launch} server_pid={server_pid}"
    ));
    let _com = ComApartment::mta()?;
    match unsafe { AllowSetForegroundWindow(server_pid) } {
        Ok(()) => log_line("AllowSetForegroundWindow 返回 S_OK"),
        Err(err) => log_line(&format!("AllowSetForegroundWindow 失败: {err}")),
    }
    let guid = guid_from_hex(hex)?;
    log_line("activation-client CoCreateInstance CLSCTX_LOCAL_SERVER");
    let callback: windows::Win32::UI::Notifications::INotificationActivationCallback = unsafe {
        windows::Win32::System::Com::CoCreateInstance(
            &guid,
            None,
            windows::Win32::System::Com::CLSCTX_LOCAL_SERVER,
        )
    }
    .map_err(|err| crate::util::win_err("CoCreateInstance", err))?;
    log_line("activation-client 已得到回调，调用 Activate");
    call_activate(&callback, launch)?;
    println!("activation-client ok aumid={AUMID} launch={launch}");
    Ok(())
}

fn located(id: &'static str) -> impl FnMut() -> bool {
    move || {
        panel::current_launch().as_deref() == Some(id)
            && panel::current_title().contains(&format!("已定位 {id}"))
            && panel::current_status().contains(&format!("已定位 {id}"))
            && panel::is_main_visible()
    }
}

fn expect_located(id: &str) -> SpikeResult<()> {
    let title = panel::current_title();
    let status = panel::current_status();
    let launch = panel::current_launch();
    log_line(&format!(
        "定位检查 launch={launch:?} title={title} status={status} visible={}",
        panel::is_main_visible()
    ));
    if launch.as_deref() != Some(id) {
        return Err(SpikeError::new(format!("launch 不是 {id}：{launch:?}")));
    }
    if !title.contains(&format!("已定位 {id}")) || !status.contains(&format!("已定位 {id}")) {
        return Err(SpikeError::new(format!(
            "面板文字不是「已定位 {id}」。title={title} status={status}"
        )));
    }
    if !panel::is_main_visible() {
        return Err(SpikeError::new("定位时 IsWindowVisible 为 false"));
    }
    Ok(())
}

fn spawn_client(clsid: u128, launch: &str) -> SpikeResult<Child> {
    let exe = exe_path()?;
    log_line(&format!(
        "启动 activation-client exe={} clsid={} launch={launch}",
        exe.display(),
        hex_clsid(clsid)
    ));
    Command::new(exe)
        .arg("activation-client")
        .arg(hex_clsid(clsid))
        .arg(launch)
        .arg(std::process::id().to_string())
        .spawn()
        .map_err(|err| SpikeError::new(format!("无法启动 activation-client: {err}")))
}

fn wait_child(child: &mut Child, timeout: Duration) -> SpikeResult<std::process::ExitStatus> {
    let start = std::time::Instant::now();
    loop {
        panel::pump_until(Duration::from_millis(50), || false)?;
        if let Some(status) = child
            .try_wait()
            .map_err(|err| SpikeError::new(format!("等待子进程失败: {err}")))?
        {
            return Ok(status);
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(SpikeError::new(
                "activation-client 退出超时，已结束该子进程",
            ));
        }
    }
}

fn other_toast_pids() -> SpikeResult<Vec<u32>> {
    let current = std::process::id();
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map_err(|err| crate::util::win_err("CreateToolhelp32Snapshot", err))?;
    let _close = Snapshot(snapshot);
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..PROCESSENTRY32W::default()
    };
    let mut found = Vec::new();
    if unsafe { Process32FirstW(snapshot, &mut entry) }.is_err() {
        return Ok(found);
    }
    loop {
        let name = crate::util::string_from_wide(&entry.szExeFile);
        if name.eq_ignore_ascii_case("toast.exe") && entry.th32ProcessID != current {
            found.push(entry.th32ProcessID);
        }
        if unsafe { Process32NextW(snapshot, &mut entry) }.is_err() {
            break;
        }
    }
    Ok(found)
}

struct Snapshot(HANDLE);

impl Drop for Snapshot {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

struct KillChild(Child);

impl Drop for KillChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            log_line("结束尚未退出的 activation-client 子进程");
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

struct ActivationGuard {
    armed: bool,
}

impl ActivationGuard {
    fn arm() -> Self {
        Self { armed: true }
    }
}

impl Drop for ActivationGuard {
    fn drop(&mut self) {
        if self.armed {
            panel::destroy_main();
            activator::revoke_class();
        }
    }
}

pub fn log_process(context: &str) {
    let args = std::env::args().collect::<Vec<_>>();
    log_line(&format!("{context} argv={args:?}"));
    match exe_path() {
        Ok(path) => log_line(&format!("{context} exe={}", path.display())),
        Err(err) => log_line(&format!("{context} exe 读取失败: {err}")),
    }
    log_line(&format!(
        "{context} AUMID={AUMID} CLSID={}",
        hex_clsid(ACTIVATOR_CLSID_U128)
    ));
    unsafe {
        let mut info = std::mem::zeroed::<STARTUPINFOW>();
        info.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        GetStartupInfoW(&mut info);
        log_line(&format!(
            "{context} STARTUPINFO flags=0x{:X} wShowWindow={}",
            info.dwFlags.0, info.wShowWindow
        ));
    }
}
