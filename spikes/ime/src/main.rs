//! Slint 文本框的中文输入法验证程序。
//!
//! 窗口只记录按键和预编辑是否到达应用，不把这些按键吃掉。

use std::io::Write;
use std::path::PathBuf;

slint::include_modules!();

const UI_LOG_LINES: usize = 40;

fn main() -> Result<(), slint::PlatformError> {
    let ui = MainWindow::new()?;
    let log_path = std::env::temp_dir().join("lanwork-ime-spike.log");
    if let Err(error) = std::fs::write(&log_path, "") {
        eprintln!("无法清空日志 {log_path:?}: {error}");
    }
    ui.set_log_path(format!("日志文件 {}", log_path.display()).as_str().into());

    let log = Log {
        path: log_path,
        ui: ui.as_weak(),
    };
    log.line(&startup_line());
    wire(&ui, &log);

    if std::env::var_os("LANWORK_IME_SPIKE_SMOKE").is_some() {
        slint::Timer::single_shot(std::time::Duration::from_millis(400), || {
            let _ = slint::quit_event_loop();
        });
    }

    ui.run()
}

fn wire(ui: &MainWindow, log: &Log) {
    ui.on_request_close(|| {
        let _ = slint::quit_event_loop();
    });

    let log_key = log.clone();
    ui.on_search_key(move |key, preedit| {
        on_key(&log_key, "搜索条", &key, &preedit);
    });

    let log_accepted = log.clone();
    ui.on_search_accepted(move |preedit, text| {
        let Some(ui) = log_accepted.ui.upgrade() else {
            return;
        };
        let count = ui.get_search_accepted_count() + 1;
        ui.set_search_accepted_count(count);
        log_accepted.line(&format!(
            "搜索条 accepted 第 {count} 次 preedit={} text={}",
            show_preedit(&preedit),
            show_text(&text),
        ));
    });

    let log_edited = log.clone();
    ui.on_search_edited(move |preedit, text| {
        log_edited.line(&format!(
            "搜索条 文本变为 preedit={} text={}",
            show_preedit(&preedit),
            show_text(&text),
        ));
    });

    let log_preedit = log.clone();
    ui.on_search_preedit(move |preedit| {
        log_preedit.line(&format!("搜索条 preedit 变为 {}", show_preedit(&preedit)));
    });

    let log_note_key = log.clone();
    ui.on_note_key(move |key, preedit| {
        on_key(&log_note_key, "便签", &key, &preedit);
    });

    let log_note_edited = log.clone();
    ui.on_note_edited(move |preedit, text| {
        log_note_edited.line(&format!(
            "便签 文本变为 preedit={} text={}",
            show_preedit(&preedit),
            show_text(&text),
        ));
    });

    let log_note_preedit = log.clone();
    ui.on_note_preedit(move |preedit| {
        log_note_preedit.line(&format!("便签 preedit 变为 {}", show_preedit(&preedit)));
    });
}

fn on_key(log: &Log, field: &str, key: &str, preedit: &str) {
    let Some(ui) = log.ui.upgrade() else {
        return;
    };
    if is_enter(key) {
        if field == "搜索条" {
            ui.set_search_enter_count(ui.get_search_enter_count() + 1);
        } else {
            ui.set_note_enter_count(ui.get_note_enter_count() + 1);
        }
    }
    if is_shortcut_digit(key) {
        if field == "搜索条" {
            ui.set_search_digit_count(ui.get_search_digit_count() + 1);
        } else {
            ui.set_note_digit_count(ui.get_note_digit_count() + 1);
        }
    }
    log.line(&format!(
        "{field} 按键 {} preedit={}",
        key_label(key),
        show_preedit(preedit),
    ));
}

fn startup_line() -> String {
    let backend = std::env::var("SLINT_BACKEND").unwrap_or_else(|_| "未设置".to_string());
    let build = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    format!("启动 Slint 1.18.1 构建 {build} SLINT_BACKEND={backend}")
}

#[derive(Clone)]
struct Log {
    path: PathBuf,
    ui: slint::Weak<MainWindow>,
}

impl Log {
    fn line(&self, line: &str) {
        eprintln!("{line}");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
        if let Some(ui) = self.ui.upgrade() {
            let next = push_log(&ui.get_log_text(), line, UI_LOG_LINES);
            ui.set_log_text(next.into());
        }
    }
}

fn push_log(existing: &str, line: &str, max_lines: usize) -> String {
    let mut lines = Vec::with_capacity(max_lines);
    if max_lines == 0 {
        return String::new();
    }
    lines.push(line);
    for existing_line in existing.lines() {
        if lines.len() >= max_lines {
            break;
        }
        lines.push(existing_line);
    }
    lines.join("\n")
}

fn is_enter(text: &str) -> bool {
    text == "\n" || text == "\r"
}

fn is_shortcut_digit(text: &str) -> bool {
    matches!(text, "1" | "2" | "3" | "4" | "5")
}

fn key_label(text: &str) -> String {
    let codes: Vec<String> = text
        .chars()
        .map(|ch| format!("U+{code:04X}", code = u32::from(ch)))
        .collect();
    let visible = if is_enter(text) {
        "Enter".to_string()
    } else if text.chars().all(|ch| !ch.is_control()) && !text.is_empty() {
        text.to_string()
    } else {
        "控制或特殊键".to_string()
    };
    format!("{visible} ({})", codes.join(" "))
}

fn show_preedit(preedit: &str) -> String {
    if preedit.is_empty() {
        "（空）".to_string()
    } else {
        format!("「{preedit}」")
    }
}

fn show_text(text: &str) -> String {
    if text.is_empty() {
        "（空）".to_string()
    } else {
        format!("「{text}」")
    }
}

#[cfg(test)]
mod tests {
    use super::{is_enter, is_shortcut_digit, key_label, push_log, show_preedit};

    #[test]
    fn enter_and_shortcut_digits() {
        assert!(is_enter("\n"));
        assert!(is_enter("\r"));
        assert!(!is_enter("1"));
        assert!(is_shortcut_digit("1"));
        assert!(is_shortcut_digit("5"));
        assert!(!is_shortcut_digit("6"));
        assert!(!is_shortcut_digit("１"));
    }

    #[test]
    fn key_label_names_enter() {
        assert_eq!(key_label("\n"), "Enter (U+000A)");
        assert_eq!(key_label("1"), "1 (U+0031)");
    }

    #[test]
    fn empty_preedit_is_visible() {
        assert_eq!(show_preedit(""), "（空）");
        assert_eq!(show_preedit("ni"), "「ni」");
    }

    #[test]
    fn push_log_keeps_newest_lines() {
        let once = push_log("", "a", 2);
        let twice = push_log(&once, "b", 2);
        let thrice = push_log(&twice, "c", 2);
        assert_eq!(thrice, "c\nb");
        assert_eq!(push_log("a", "b", 0), "");
    }
}
