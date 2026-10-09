mod activator;
mod check;
mod model;
mod notify;
mod panel;
mod registry;
mod tray;
mod util;

use std::process::ExitCode;

use model::{
    DISPLAY_NAME, Delivery, RegistrationKind, classify, delivery, format_facts, sample_title,
};
use notify::{format_outcome, set_process_aumid, show_toasts};
use registry::{Mode, inspect, register, remove_registration_artifacts, unregister};
use util::{ComApartment, SpikeError, SpikeResult, enable_utf8_console, log_line, log_path};

fn main() -> ExitCode {
    enable_utf8_console();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("错误：{err}");
            ExitCode::from(1)
        }
    }
}

fn run() -> SpikeResult<()> {
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|arg| is_embedding(arg)) || args.is_empty() {
        return serve();
    }
    let command = args.remove(0);
    match command.as_str() {
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        "status" => registry::print_status(),
        "register" => register_command(&args),
        "unregister" => {
            let _com = ComApartment::new()?;
            let _ = set_process_aumid();
            unregister()?;
            log_line("已删除本 spike 的快捷方式、HKCU 注册项，并尝试清除该 AUMID 的通知历史。");
            registry::print_status()
        }
        "serve" => serve(),
        "show" => show_command(&args),
        "clear-history" => {
            let _com = ComApartment::new()?;
            let _ = set_process_aumid();
            notify::clear_history()?;
            log_line("已清除该 AUMID 的通知历史。");
            Ok(())
        }
        "activation-check" => check::activation_check(),
        "activation-client" => check::activation_client(&args),
        "self-check" => self_check(),
        other => Err(SpikeError::new(format!(
            "未知命令：{other}\n{}",
            help_text()
        ))),
    }
}

fn serve() -> SpikeResult<()> {
    check::log_process("serve");
    log_line(&format!("日志：{}", log_path().display()));
    let _com = ComApartment::new()?;
    set_process_aumid()?;
    set_dpi();
    match inspect() {
        Ok(facts) => log_line(&format!("启动时注册读回：{}", format_facts(&facts))),
        Err(err) => log_line(&format!("启动时读取注册失败: {err}")),
    }
    activator::register_class()?;
    let _guard = ServeGuard;
    panel::startup()?;
    activator::resume_class()?;
    panel::message_loop()
}

struct ServeGuard;

impl Drop for ServeGuard {
    fn drop(&mut self) {
        panel::destroy_main();
        activator::revoke_class();
    }
}

fn register_command(args: &[String]) -> SpikeResult<()> {
    let _com = ComApartment::new()?;
    let mode = match args {
        [mode] if mode == "installed" => Mode::Installed,
        [mode] if mode == "portable" => Mode::Portable,
        _ => {
            return Err(SpikeError::new(
                "用法：register installed    或    register portable",
            ));
        }
    };
    register(mode)?;
    log_line("注册完成。下面是读回结果。");
    registry::print_status()
}

fn show_command(args: &[String]) -> SpikeResult<()> {
    let request = parse_show(args)?;
    let _com = ComApartment::new()?;
    set_process_aumid()?;
    let facts = inspect()?;
    println!("{}", format_facts(&facts));
    let title = sample_title(&request.id).unwrap_or("未在列表中");
    match delivery(classify(&facts)) {
        Delivery::TrayOnly => {
            log_line("注册缺失，不调用 ToastNotificationManager.Show，只显示托盘徽标。");
            tray::show_badge_for(15)?;
            log_line("托盘徽标已移除，进程正常退出。");
            Ok(())
        }
        Delivery::Toast => {
            let outcome = show_toasts(
                &request.id,
                "待办到期",
                &format!("{} {title}", request.id),
                request.repeat,
                request.tag.as_deref(),
            );
            println!("{}", format_outcome(&outcome));
            if outcome.results.iter().all(|result| result.is_ok()) {
                log_line("Show 已返回。通知是否出现在屏幕上，需要看系统通知。");
                Ok(())
            } else {
                log_line("Show 失败，退回托盘徽标，进程仍正常退出。");
                tray::show_badge_for(15)?;
                Ok(())
            }
        }
    }
}

fn self_check() -> SpikeResult<()> {
    let _com = ComApartment::new()?;
    set_process_aumid()?;
    let _cleanup = Cleanup;
    println!("=== 通知 spike 自动检查 ===");
    check::activation_check()?;
    println!("--- 激活链路自检已完成，开始注册表往返 ---");
    println!("程序：{}", util::exe_path()?.display());
    match notify::notification_state_line() {
        Ok(line) => println!("SHQueryUserNotificationState = {line}"),
        Err(err) => println!("SHQueryUserNotificationState 失败: {err}"),
    }

    remove_registration_artifacts()?;
    let _ = notify::clear_history();
    let facts = inspect()?;
    expect_kind(&facts, RegistrationKind::Missing, "清理后")?;

    println!("--- 未注册时调用 Show ---");
    let unregistered = show_toasts("todo-001", "Lanwork 自动检查", "未注册", 1, None);
    println!("{}", format_outcome(&unregistered));
    let _ = notify::clear_history();

    println!("--- 未注册时的托盘徽标往返 ---");
    tray::roundtrip()?;
    println!("Shell_NotifyIcon ADD/MODIFY/DELETE 均返回 TRUE");

    println!("--- 安装版注册读回 ---");
    register(Mode::Installed)?;
    let facts = inspect()?;
    println!("{}", format_facts(&facts));
    expect_kind(&facts, RegistrationKind::Installed, "安装版注册后")?;
    run_show_matrix("安装版")?;
    unregister()?;
    expect_kind(&inspect()?, RegistrationKind::Missing, "安装版注销后")?;

    println!("--- 便携版注册读回 ---");
    register(Mode::Portable)?;
    let facts = inspect()?;
    println!("{}", format_facts(&facts));
    expect_kind(&facts, RegistrationKind::Portable, "便携版注册后")?;
    if facts.shortcut_present || facts.custom_activator.is_some() || facts.local_server.is_some() {
        return Err(SpikeError::new(
            "便携版写出了快捷方式、CustomActivator 或 LocalServer32",
        ));
    }
    run_show_matrix("便携版")?;
    unregister()?;
    expect_kind(&inspect()?, RegistrationKind::Missing, "便携版注销后")?;
    println!("=== 自动检查结束：注册往返和清理已完成。屏幕显示与点击未在本命令中观察。 ===");
    Ok(())
}

fn run_show_matrix(label: &str) -> SpikeResult<()> {
    let _ = notify::clear_history();
    println!("--- {label} 单次 Show，无 Tag ---");
    println!(
        "{}",
        format_outcome(&show_toasts(
            "todo-001",
            "Lanwork 自动检查",
            "单次",
            1,
            None
        ))
    );
    let _ = notify::clear_history();
    println!("--- {label} 同一 id 连续 3 次，无 Tag ---");
    println!(
        "{}",
        format_outcome(&show_toasts(
            "todo-001",
            "Lanwork 自动检查",
            "无 Tag",
            3,
            None
        ))
    );
    let _ = notify::clear_history();
    println!("--- {label} 同一 id 连续 3 次，Tag=todo-001 Group=lanwork-spike ---");
    println!(
        "{}",
        format_outcome(&show_toasts(
            "todo-001",
            "Lanwork 自动检查",
            "有 Tag",
            3,
            Some("todo-001"),
        ))
    );
    let _ = notify::clear_history();
    Ok(())
}

fn expect_kind(
    facts: &model::RegistrationFacts,
    expected: RegistrationKind,
    when: &str,
) -> SpikeResult<()> {
    let actual = classify(facts);
    if actual == expected {
        println!("{when}：{}", model::kind_label(actual));
        Ok(())
    } else {
        Err(SpikeError::new(format!(
            "{when}：期望 {}，实际 {}\n{}",
            model::kind_label(expected),
            model::kind_label(actual),
            format_facts(facts)
        )))
    }
}

struct Cleanup;

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Err(err) = unregister() {
            eprintln!("自动检查清理失败：{err}");
        }
    }
}

struct ShowRequest {
    id: String,
    repeat: u32,
    tag: Option<String>,
}

fn parse_show(args: &[String]) -> SpikeResult<ShowRequest> {
    let Some(id) = args.first() else {
        return Err(SpikeError::new(
            "用法：show <待办 id> [--repeat N] [--tag TAG]",
        ));
    };
    if id.is_empty() || id.starts_with('-') {
        return Err(SpikeError::new("show 的第一个参数必须是待办 id"));
    }
    let mut repeat = 1u32;
    let mut tag = None;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--repeat" => {
                index += 1;
                let raw = args
                    .get(index)
                    .ok_or_else(|| SpikeError::new("--repeat 后面要有 1 到 5 的数字"))?;
                repeat = raw
                    .parse::<u32>()
                    .map_err(|_| SpikeError::new("--repeat 后面要有 1 到 5 的数字"))?;
                if !(1..=5).contains(&repeat) {
                    return Err(SpikeError::new("--repeat 只允许 1 到 5"));
                }
            }
            "--tag" => {
                index += 1;
                let raw = args
                    .get(index)
                    .ok_or_else(|| SpikeError::new("--tag 后面要有不含空白的标记"))?
                    .clone();
                if raw.is_empty() || raw.chars().any(char::is_whitespace) {
                    return Err(SpikeError::new("--tag 不能为空，也不能含空白"));
                }
                tag = Some(raw);
            }
            other => return Err(SpikeError::new(format!("无法识别的参数：{other}"))),
        }
        index += 1;
    }
    Ok(ShowRequest {
        id: id.clone(),
        repeat,
        tag,
    })
}

pub(crate) fn set_dpi() {
    use windows::Win32::UI::HiDpi::{
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
    };
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn is_embedding(arg: &str) -> bool {
    arg.eq_ignore_ascii_case("-Embedding") || arg.eq_ignore_ascii_case("/Embedding")
}

fn print_help() {
    println!("{}", help_text());
}

fn help_text() -> String {
    format!(
        "\
Lanwork 通知技术验证（{DISPLAY_NAME}）
AUMID：{aumid}

  cargo run -p toast --release -- register installed
  cargo run -p toast --release -- register portable
  cargo run -p toast --release -- unregister
  cargo run -p toast --release -- status
  cargo run -p toast --release -- serve
  cargo run -p toast --release -- show todo-001
  cargo run -p toast --release -- show todo-001 --repeat 3
  cargo run -p toast --release -- show todo-001 --repeat 3 --tag todo-001
  cargo run -p toast --release -- clear-history
  cargo run -p toast --release -- activation-check
  cargo run -p toast --release -- self-check

无参数或 -Embedding 会打开模拟面板。注册只写当前用户的 HKCU 和开始菜单快捷方式。
注销会删掉这些项。不要用管理员权限运行。
activation-check 不点击通知，也不写 HKCU：它自己 CoCreateInstance 激活器并调用 Activate。
若已有其他 toast.exe，它不会注册产品 CLSID。",
        aumid = model::AUMID,
    )
}
