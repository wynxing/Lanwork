//! Lanwork 程序外壳。
//!
//! 非 Windows 上只说明平台并退出。
//! 发布构建没有 `debug_assertions`，在 Windows 上使用 GUI 子系统，不分配控制台。
//! 调试构建保留控制台子系统。

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use lanwork_core as _;

slint::include_modules!();

#[cfg(test)]
mod float_layout_tests;

#[cfg(windows)]
mod bar_win;
#[cfg(windows)]
mod host;
#[cfg(windows)]
mod hotcorner;
#[cfg(windows)]
mod instance;
#[cfg(windows)]
mod note_editor;
#[cfg(windows)]
mod note_float;
#[cfg(windows)]
mod notes_page;
#[cfg(windows)]
mod panel;
#[cfg(windows)]
mod platform;
#[cfg(windows)]
mod registry;
#[cfg(windows)]
mod searchbar;
#[cfg(windows)]
mod shelf_dnd;
#[cfg(windows)]
mod shelf_float;
#[cfg(windows)]
mod shelf_refs;
#[cfg(windows)]
mod shelves_page;
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
    fn release_windows_build_requests_the_gui_subsystem() {
        let source = include_str!("main.rs");
        assert!(source.contains("windows_subsystem = \"windows\""));
        assert!(source.contains("not(debug_assertions)"));
        assert!(source.contains("all(windows, not(debug_assertions))"));
    }

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
        let sources = [
            ("panel.slint", include_str!("../ui/panel.slint")),
            ("notespage.slint", include_str!("../ui/notespage.slint")),
            ("notefloat.slint", include_str!("../ui/notefloat.slint")),
            ("shelvespage.slint", include_str!("../ui/shelvespage.slint")),
            ("shelffloat.slint", include_str!("../ui/shelffloat.slint")),
        ];
        for (name, source) in sources {
            for forbidden in ["今日", "focus-page", "FocusPage"] {
                assert!(!source.contains(forbidden), "{name} mentions {forbidden}");
            }
        }
    }

    #[test]
    fn windows_use_the_app_icon() {
        let icon = "icon: @image-url(\"../assets/lanwork-256.png\");";
        for (name, source) in [
            ("searchbar.slint", include_str!("../ui/searchbar.slint")),
            ("panel.slint", include_str!("../ui/panel.slint")),
            ("notefloat.slint", include_str!("../ui/notefloat.slint")),
            ("shelffloat.slint", include_str!("../ui/shelffloat.slint")),
        ] {
            assert!(source.contains(icon), "{name} 没有窗口图标");
        }
        let png = include_bytes!("../assets/lanwork-256.png");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&png[12..16], b"IHDR");
        let width = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let height = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!((width, height), (256, 256));
    }

    #[test]
    fn note_inputs_expose_the_ime_composition_text() {
        for (name, source) in [
            ("noteeditor.slint", include_str!("../ui/noteeditor.slint")),
            ("widgets.slint", include_str!("../ui/widgets.slint")),
        ] {
            assert!(source.contains("preedit-text"), "{name} ignores preedit");
        }
    }
}
