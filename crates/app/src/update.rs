//! 设置页上的检查更新。决定在 `lanwork_core::update`，这里负责下载、托盘和退出后安装。
//!
//! 不写数据目录。安装参数里没有 `/DIR`，也没有数据目录。

use std::cell::{Cell, RefCell};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use lanwork_core::shell::QuitSource;
use lanwork_core::storage::Store;
use lanwork_core::update::{
    ButtonAction, CHECK_INTERVAL, CheckOutcome, ContentHash, Edition, GetBytes,
    MAX_INSTALLER_BYTES, Phase, STARTUP_DELAY, TEXT_CHECK_FAILED, TEXT_DOWNLOAD_FAILED,
    TEXT_INSTALL_FAILED, TEXT_OPEN_FAILED, TransportError, UpdateSession, current_version,
    download_path, edition_of, install_failure_log, install_failure_marker, pinned_installer_url,
    present, release_page_allowed, review_download, run_check, update_helper_script, url_allowed,
};
use slint::ComponentHandle;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, CreateProcessW,
    PROCESS_INFORMATION, STARTUPINFOW,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{HSTRING, PCWSTR, PWSTR};

use crate::panel::with_settings;
use crate::update_http::{self, WinHttp};
use crate::{Panel, SettingsPageData};

struct Pending {
    installer: PathBuf,
    app: PathBuf,
}

thread_local! {
    static SESSION: RefCell<UpdateSession> = RefCell::new(UpdateSession::new());
    static STORE: RefCell<Option<Store>> = const { RefCell::new(None) };
    static TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
    static GEN: Cell<u64> = const { Cell::new(0) };
    static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
}

/// 托盘接好之后调用。先把当前版本推到设置页，再等事件循环起来后检查一次。
pub(crate) fn start(store: Store) {
    if let Some(message) = consume_install_failure(&store) {
        store.log_warn(&message);
        SESSION.with(|cell| cell.borrow_mut().note_install_failure());
    }
    STORE.with(|cell| *cell.borrow_mut() = Some(store));
    push_ui();
    slint::Timer::single_shot(STARTUP_DELAY, || begin_check(false));
}

/// 设置页创建时调用。那时控制器可能还没放进表里，所以直接写这个窗口。
pub(crate) fn push_into(ui: &Panel) {
    apply_to(ui);
}

pub(crate) fn on_clicked() {
    let action = SESSION
        .with(|cell| present(current_version(), current_edition(), cell.borrow().phase()).action);
    match action {
        ButtonAction::Check => begin_check(true),
        ButtonAction::Install => begin_install(),
        ButtonAction::OpenRelease => open_release(),
        ButtonAction::None => {}
    }
}

/// 退出已经决定、便签也写完之后调用。没有待安装的包时成功。启动失败时安装包仍留在临时目录。
pub(crate) fn spawn_pending_install() -> Result<(), ()> {
    let pending = PENDING.with(|cell| cell.borrow_mut().take());
    let Some(pending) = pending else {
        return Ok(());
    };
    let Some(store) = STORE.with(|cell| cell.borrow().clone()) else {
        let _ = std::fs::remove_file(&pending.installer);
        return Err(());
    };
    let Some(marker) =
        install_failure_marker(store.cache_dir(), &store.data_dir(), &std::env::temp_dir())
    else {
        let _ = std::fs::remove_file(&pending.installer);
        return Err(());
    };
    if let Some(parent) = marker.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let Some(script) = update_helper_script(
        std::process::id(),
        &pending.installer,
        &pending.app,
        &marker,
    ) else {
        let _ = std::fs::remove_file(&pending.installer);
        return Err(());
    };
    if spawn_helper(&script).is_err() {
        let _ = std::fs::remove_file(&pending.installer);
        return Err(());
    }
    Ok(())
}

/// 助手没有启动。进程留下，设置页显示安装失败，原来的更新还可以再点。
pub(crate) fn install_spawn_failed() {
    log_warn("安装失败");
    SESSION.with(|cell| cell.borrow_mut().fail(TEXT_INSTALL_FAILED));
    push_ui();
}

fn consume_install_failure(store: &Store) -> Option<String> {
    let marker =
        install_failure_marker(store.cache_dir(), &store.data_dir(), &std::env::temp_dir())?;
    if !marker.is_file() {
        return None;
    }
    let text = std::fs::read_to_string(&marker).unwrap_or_default();
    let _ = std::fs::remove_file(&marker);
    Some(install_failure_log(&text))
}

fn begin_check(manual: bool) {
    let busy = SESSION.with(|cell| is_busy(cell.borrow().phase()));
    if busy {
        return;
    }
    let generation = bump();
    SESSION.with(|cell| cell.borrow_mut().begin_check());
    push_ui();
    let store = STORE.with(|cell| cell.borrow().clone());
    let spawned = std::thread::Builder::new()
        .name("lanwork-update".to_owned())
        .spawn(move || {
            let outcome = run_check(
                &Reporting {
                    store: store.clone(),
                },
                current_version(),
            );
            let _ = slint::invoke_from_event_loop(move || {
                finish_check(generation, outcome, manual, store)
            });
        });
    if spawned.is_err() {
        finish_check(
            generation,
            CheckOutcome::Failed(TEXT_CHECK_FAILED),
            manual,
            STORE.with(|cell| cell.borrow().clone()),
        );
    }
}

fn finish_check(generation: u64, outcome: CheckOutcome, manual: bool, store: Option<Store>) {
    if GEN.with(|cell| cell.get()) != generation {
        return;
    }
    if let CheckOutcome::Failed(_) = &outcome
        && let Some(store) = &store
    {
        store.log_warn(TEXT_CHECK_FAILED);
    }
    SESSION.with(|cell| cell.borrow_mut().finish_check(outcome, manual));
    arm_daily();
    push_ui();
}

fn begin_install() {
    if SESSION.with(|cell| is_busy(cell.borrow().phase())) {
        return;
    }
    let update = SESSION.with(|cell| cell.borrow().current_update().cloned());
    let Some(update) = update else {
        return;
    };
    let Some(installer) = update.installer.clone() else {
        return;
    };
    if installer.url != pinned_installer_url(update.version) || !url_allowed(&installer.url) {
        fail_download(TEXT_DOWNLOAD_FAILED);
        return;
    }
    let Some(store) = STORE.with(|cell| cell.borrow().clone()) else {
        fail_download(TEXT_DOWNLOAD_FAILED);
        return;
    };
    let Some(path) = download_path(
        &std::env::temp_dir(),
        &store.data_dir(),
        store.cache_dir(),
        update.version,
    ) else {
        store.log_warn("下载失败：写入临时文件失败");
        fail_download(TEXT_DOWNLOAD_FAILED);
        return;
    };
    let Ok(app) = std::env::current_exe() else {
        store.log_warn(TEXT_INSTALL_FAILED);
        SESSION.with(|cell| cell.borrow_mut().fail(TEXT_INSTALL_FAILED));
        push_ui();
        return;
    };
    let generation = bump();
    SESSION.with(|cell| cell.borrow_mut().begin_download(update));
    push_ui();
    let url = installer.url.clone();
    let expected = installer.sha256.clone();
    let spawned = std::thread::Builder::new()
        .name("lanwork-update".to_owned())
        .spawn(move || download_then_install(generation, url, expected, path, app, store));
    if spawned.is_err() {
        fail_download(TEXT_DOWNLOAD_FAILED);
    }
}

fn download_then_install(
    generation: u64,
    url: String,
    expected: String,
    path: PathBuf,
    app: PathBuf,
    store: Store,
) {
    let mut guard = DeleteOnDrop::new(path.clone());
    let written = write_installer(&path, &url, generation);
    let outcome = match written {
        Ok((digest, bytes)) => review_download(&expected, &digest, bytes).map(|()| digest),
        Err(err) => {
            store.log_warn(&format!("下载失败：{err}"));
            Err(TEXT_DOWNLOAD_FAILED)
        }
    };
    match outcome {
        Ok(_) => {
            guard.defuse();
            let _ = slint::invoke_from_event_loop(move || {
                if GEN.with(|cell| cell.get()) != generation {
                    let _ = std::fs::remove_file(&path);
                    return;
                }
                SESSION.with(|cell| cell.borrow_mut().begin_verify());
                push_ui();
                let path = path.clone();
                let app = app.clone();
                slint::Timer::single_shot(Duration::ZERO, move || {
                    if GEN.with(|cell| cell.get()) != generation {
                        let _ = std::fs::remove_file(&path);
                        return;
                    }
                    PENDING.with(|cell| {
                        *cell.borrow_mut() = Some(Pending {
                            installer: path,
                            app,
                        });
                    });
                    SESSION.with(|cell| cell.borrow_mut().begin_install_phase());
                    push_ui();
                    crate::host::request_quit_from(QuitSource::Update);
                });
            });
        }
        Err(message) => {
            drop(guard);
            if message != TEXT_DOWNLOAD_FAILED {
                store.log_warn(message);
            }
            let _ = slint::invoke_from_event_loop(move || {
                if GEN.with(|cell| cell.get()) != generation {
                    return;
                }
                SESSION.with(|cell| cell.borrow_mut().fail(message));
                push_ui();
            });
        }
    }
}

fn write_installer(
    path: &Path,
    url: &str,
    generation: u64,
) -> Result<(String, u64), TransportError> {
    let mut file = File::create(path).map_err(|_| TransportError::Disk)?;
    let mut hash = ContentHash::new();
    let mut received = 0u64;
    let mut last_mark = None;
    update_http::pull(
        url,
        MAX_INSTALLER_BYTES,
        "application/octet-stream",
        |chunk, total| {
            file.write_all(chunk).map_err(|_| TransportError::Disk)?;
            hash.update(chunk);
            received = received.saturating_add(u64::try_from(chunk.len()).unwrap_or(u64::MAX));
            if progress_changed(received, total, &mut last_mark) {
                let got = received;
                let _ =
                    slint::invoke_from_event_loop(move || apply_progress(generation, got, total));
            }
            Ok(())
        },
    )?;
    file.sync_all().map_err(|_| TransportError::Disk)?;
    Ok((hash.finish_hex(), received))
}

fn progress_changed(received: u64, total: Option<u64>, last: &mut Option<u64>) -> bool {
    let mark = match total {
        Some(total) if total > 0 => received.saturating_mul(100) / total,
        _ => received / (256 * 1024),
    };
    if *last == Some(mark) {
        return false;
    }
    *last = Some(mark);
    true
}

fn apply_progress(generation: u64, received: u64, total: Option<u64>) {
    if GEN.with(|cell| cell.get()) != generation {
        return;
    }
    let updated = SESSION.with(|cell| {
        let mut session = cell.borrow_mut();
        if !matches!(session.phase(), Phase::Downloading { .. }) {
            return false;
        }
        session.download_progress(received, total);
        true
    });
    if updated {
        push_ui();
    }
}

fn open_release() {
    let url = SESSION.with(|cell| {
        cell.borrow()
            .current_update()
            .map(|update| update.release_url.clone())
    });
    let Some(url) = url else {
        return;
    };
    if !release_page_allowed(&url) || !shell_open(&url) {
        log_warn(TEXT_OPEN_FAILED);
        SESSION.with(|cell| cell.borrow_mut().fail(TEXT_OPEN_FAILED));
        push_ui();
    }
}

fn fail_download(message: &'static str) {
    log_warn(message);
    SESSION.with(|cell| cell.borrow_mut().fail(message));
    push_ui();
}

fn push_ui() {
    with_settings(|_page, ui| apply_to(ui));
}

fn apply_to(ui: &Panel) {
    let view =
        SESSION.with(|cell| present(current_version(), current_edition(), cell.borrow().phase()));
    let data = ui.global::<SettingsPageData>();
    data.set_version_text(view.version_text.into());
    data.set_update_status(view.status.into());
    data.set_update_status_error(view.status_error);
    data.set_update_notes(view.notes.into());
    data.set_update_notes_visible(view.notes_visible);
    data.set_update_button(view.button.into());
    data.set_update_enabled(view.button_enabled);
    crate::host::set_update_dot(view.tray);
}

fn current_edition() -> Edition {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(edition_of))
        .unwrap_or(Edition::Portable)
}

fn arm_daily() {
    TIMER.with(|cell| {
        let mut slot = cell.borrow_mut();
        let timer = slot.get_or_insert_with(slint::Timer::default);
        timer.start(slint::TimerMode::SingleShot, CHECK_INTERVAL, || {
            begin_check(false);
        });
    });
}

fn bump() -> u64 {
    GEN.with(|cell| {
        let next = cell.get().wrapping_add(1);
        cell.set(next);
        next
    })
}

fn is_busy(phase: &Phase) -> bool {
    matches!(
        phase,
        Phase::Checking { .. }
            | Phase::Downloading { .. }
            | Phase::Verifying(_)
            | Phase::Installing(_)
    )
}

fn log_warn(message: &str) {
    if let Some(store) = STORE.with(|cell| cell.borrow().clone()) {
        store.log_warn(message);
    }
}

struct Reporting {
    store: Option<Store>,
}

impl GetBytes for Reporting {
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>, TransportError> {
        match WinHttp.get(url, limit) {
            Ok(body) => Ok(body),
            Err(err) => {
                if let Some(store) = &self.store {
                    store.log_warn(&format!("检查更新：{err}"));
                }
                Err(err)
            }
        }
    }
}

struct DeleteOnDrop {
    path: Option<PathBuf>,
}

impl DeleteOnDrop {
    fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn defuse(&mut self) {
        self.path.take();
    }
}

impl Drop for DeleteOnDrop {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn shell_open(url: &str) -> bool {
    let file = HSTRING::from(url);
    let result = unsafe {
        ShellExecuteW(
            None,
            windows::core::w!("open"),
            &file,
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    (result.0 as isize) > 32
}

fn spawn_helper(script: &str) -> Result<(), ()> {
    let powershell = powershell_exe();
    let Some(command) = helper_command(&powershell, script) else {
        return Err(());
    };
    if launch(&command, true).is_ok() {
        return Ok(());
    }
    log_warn("安装程序助手改在当前任务里启动");
    launch(&command, false)
}

fn powershell_exe() -> PathBuf {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
    PathBuf::from(root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe")
}

fn helper_command(powershell: &Path, script: &str) -> Option<String> {
    let exe = powershell.to_str()?;
    if exe.chars().any(|ch| ch.is_control() || ch == '"')
        || script.contains('"')
        || script.chars().any(|ch| ch.is_control())
    {
        return None;
    }
    Some(format!(
        "\"{exe}\" -NoProfile -NonInteractive -WindowStyle Hidden -Command {script}"
    ))
}

fn launch(command: &str, breakaway: bool) -> Result<(), ()> {
    let mut wide: Vec<u16> = command.encode_utf16().chain(std::iter::once(0)).collect();
    let mut info = PROCESS_INFORMATION::default();
    let startup = STARTUPINFOW {
        cb: u32::try_from(std::mem::size_of::<STARTUPINFOW>()).map_err(|_| ())?,
        ..Default::default()
    };
    let flags = if breakaway {
        CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP
    } else {
        CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP
    };
    let created = unsafe {
        CreateProcessW(
            PCWSTR::null(),
            Some(PWSTR(wide.as_mut_ptr())),
            None,
            None,
            false,
            flags,
            None,
            PCWSTR::null(),
            &startup,
            &mut info,
        )
    };
    if created.is_err() {
        return Err(());
    }
    unsafe {
        let _ = CloseHandle(info.hProcess);
        let _ = CloseHandle(info.hThread);
    }
    Ok(())
}
