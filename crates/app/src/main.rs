//! Lanwork 程序外壳。
//!
//! 非 Windows 上只说明平台并退出。

use lanwork_core as _;

slint::include_modules!();

#[cfg(windows)]
mod bar_win;
#[cfg(windows)]
mod host;
#[cfg(windows)]
mod instance;
#[cfg(windows)]
mod panel;
#[cfg(windows)]
mod platform;
#[cfg(windows)]
mod registry;
#[cfg(windows)]
mod searchbar;
#[cfg(windows)]
mod winutil;

fn main() {
    #[cfg(windows)]
    {
        let code = host::run();
        if code != 0 {
            std::process::exit(code);
        }
    }
    #[cfg(not(windows))]
    {
        eprintln!("Lanwork 只支持 Windows 11 x64");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn tray_menu_titles_are_the_shell_menu() {
        let source = include_str!("../ui/shell.slint");
        for item in lanwork_core::shell::TRAY_MENU {
            assert!(
                source.contains(&format!("title: \"{}\";", item.title)),
                "missing {}",
                item.title
            );
        }
        assert!(source.contains("tooltip: \"Lanwork\""));
        assert!(source.contains("title: \"退出\""));
    }

    #[test]
    fn panel_has_no_today_or_focus_page() {
        let source = include_str!("../ui/panel.slint");
        for forbidden in ["今日", "focus-page", "FocusPage"] {
            assert!(
                !source.contains(forbidden),
                "panel.slint mentions {forbidden}"
            );
        }
    }
}
