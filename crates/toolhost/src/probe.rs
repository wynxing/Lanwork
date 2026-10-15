//! 隐藏的凭据验证入口。只在 Windows 上读写凭据管理器。
//!
//! 不解析数据目录，也不创建用户数据文件。
//! 目标名 `Lanwork.ToolHost.Verify/probe/test-key` 只给这次人工确认用。
//! 产品在凭据管理器里的目标名仍待定，这里的前缀不是那条规则。
//! `write` 用本机持久留下条目，进程退出后仍在，直到 `clear`。

/// 验证条目的前缀。`list` 和 `clear` 只碰这一段。
pub const VERIFY_PREFIX: &str = "Lanwork.ToolHost.Verify/";

/// `write` 留下的那一条。凭据管理器里按这个名字找。
pub const VERIFY_TARGET: &str = "Lanwork.ToolHost.Verify/probe/test-key";

#[cfg(windows)]
const VERIFY_VALUE: &str = "lanwork-toolhost-verify";

/// 进程参数里出现 `--toolhost-cred` 时处理并返回退出码。否则返回 `None`，调用方照常启动。
///
/// 不读取数据目录。
#[must_use]
pub fn dispatch_from_args() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    dispatch(&args)
}

/// 与 [`dispatch_from_args`] 相同，参数由调用方给出，便于测试。
#[must_use]
pub fn dispatch(args: &[String]) -> Option<i32> {
    if args.first().map(String::as_str) != Some("--toolhost-cred") {
        return None;
    }
    let action = args.get(1).map(String::as_str).unwrap_or("");
    if args.len() > 2 {
        emit("用法：lanwork --toolhost-cred write|list|clear");
        return Some(1);
    }
    Some(run(action))
}

fn run(action: &str) -> i32 {
    #[cfg(windows)]
    {
        attach_parent_console();
        let result = match action {
            "write" => write_left_behind(),
            "list" => list_and_print(),
            "clear" => clear_verify(),
            _ => {
                emit("用法：lanwork --toolhost-cred write|list|clear");
                return 1;
            }
        };
        match result {
            Ok(()) => 0,
            Err(err) => {
                emit(&err);
                1
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = action;
        emit("工具凭据验证只在 Windows 上运行");
        1
    }
}

/// 写入并留下测试密钥。不打印原文。
#[cfg(windows)]
pub(crate) fn write_left_behind() -> Result<(), String> {
    write_persisted(VERIFY_TARGET, VERIFY_VALUE)?;
    emit(&format!("已写入 {VERIFY_TARGET}"));
    Ok(())
}

#[cfg(windows)]
pub(crate) fn write_persisted(target: &str, value: &str) -> Result<(), String> {
    crate::secret::credential_write(
        target,
        value,
        crate::secret::CredentialPersist::LocalMachine,
    )
}

#[cfg(windows)]
fn list_and_print() -> Result<(), String> {
    let names = list_targets()?;
    if names.is_empty() {
        emit("没有验证密钥");
        return Ok(());
    }
    for name in names {
        emit(&name);
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn list_targets() -> Result<Vec<String>, String> {
    crate::secret::credential_list_targets(VERIFY_PREFIX)
}

#[cfg(windows)]
pub(crate) fn clear_verify() -> Result<(), String> {
    let names = list_targets()?;
    let count = names.len();
    for name in &names {
        crate::secret::credential_delete(name)?;
    }
    emit(&format!("已清除 {count} 条"));
    Ok(())
}

#[cfg(windows)]
fn attach_parent_console() {
    use windows::Win32::System::Console::{ATTACH_PARENT_PROCESS, AllocConsole, AttachConsole};
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_err() {
            let _ = AllocConsole();
        }
    }
}

fn emit(text: &str) {
    #[cfg(windows)]
    {
        if write_console_line(text) {
            return;
        }
    }
    eprintln!("{text}");
}

#[cfg(windows)]
fn write_console_line(text: &str) -> bool {
    use windows::Win32::System::Console::{GetStdHandle, STD_OUTPUT_HANDLE, WriteConsoleW};
    let wide: Vec<u16> = text
        .encode_utf16()
        .chain(std::iter::once(u16::from(b'\n')))
        .collect();
    unsafe {
        let Ok(handle) = GetStdHandle(STD_OUTPUT_HANDLE) else {
            return false;
        };
        if handle.is_invalid() {
            return false;
        }
        let mut written = 0u32;
        WriteConsoleW(handle, &wide, Some(std::ptr::addr_of_mut!(written)), None).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::{VERIFY_PREFIX, VERIFY_TARGET, dispatch};

    #[test]
    fn ordinary_args_do_not_enter_the_probe() {
        assert!(dispatch(&[]).is_none());
        assert!(dispatch(&["--help".to_string()]).is_none());
    }

    #[test]
    fn cargo_test_args_do_not_enter_the_probe() {
        assert!(super::dispatch_from_args().is_none());
    }

    #[test]
    fn probe_source_does_not_touch_the_data_directory() {
        let source = include_str!("probe.rs");
        let source = source.split("mod tests").next().unwrap();
        assert!(!source.contains("Documents"));
        assert!(!source.contains("LANWORK_DATA_DIR"));
        assert!(!source.contains("resolve_from_process"));
        assert!(!source.contains("MayDolist"));
        assert!(source.contains(VERIFY_PREFIX));
        assert!(source.contains(VERIFY_TARGET));
    }

    #[test]
    fn product_main_checks_the_probe_before_startup() {
        let main = include_str!("../../../crates/app/src/main.rs");
        let probe = main
            .find("probe::dispatch_from_args")
            .expect("main 应先看验证参数");
        let startup = main.find("host::run").expect("main 里有启动");
        assert!(probe < startup);
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_probe_exits_without_writing() {
        let code = dispatch(&["--toolhost-cred".to_string(), "list".to_string()]);
        assert_eq!(code, Some(1));
    }

    #[cfg(windows)]
    #[test]
    fn local_credential_roundtrip_is_removed_by_the_test() {
        let target = format!("{VERIFY_PREFIX}ci/{}", std::process::id());
        let _guard = ClearGuard(target.clone());
        super::write_persisted(&target, "ci-only").expect("写入");
        let listed = super::list_targets().expect("列出");
        assert!(listed.iter().any(|name| name == &target));
        assert!(listed.iter().all(|name| !name.contains("ci-only")));
        crate::secret::credential_delete(&target).expect("删除");
        let listed = super::list_targets().expect("再列");
        assert!(listed.iter().all(|name| name != &target));
    }

    #[cfg(windows)]
    struct ClearGuard(String);

    #[cfg(windows)]
    impl Drop for ClearGuard {
        fn drop(&mut self) {
            let _ = crate::secret::credential_delete(&self.0);
        }
    }
}
