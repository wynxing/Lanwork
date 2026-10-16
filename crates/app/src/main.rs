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
#[cfg(test)]
mod menu_layout_tests;
#[cfg(test)]
mod notes_layout_tests;
#[cfg(test)]
mod settings_layout_tests;
#[cfg(test)]
mod shelves_layout_tests;
#[cfg(test)]
mod triage_layout_tests;

#[cfg(windows)]
mod bar_win;
#[cfg(windows)]
mod favicon_http;
#[cfg(windows)]
mod favicon_wic;
#[cfg(windows)]
mod file_pick;
#[cfg(windows)]
mod float_dock;
#[cfg(windows)]
mod float_frame;
#[cfg(windows)]
mod float_group;
#[cfg(windows)]
mod float_shell;
#[cfg(windows)]
mod folder_pick;
mod frame_policy;
#[cfg(windows)]
mod host;
#[cfg(windows)]
mod hotcorner;
#[cfg(windows)]
mod instance;
#[cfg(windows)]
mod keycap;
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
mod remind;
#[cfg(windows)]
mod searchbar;
#[cfg(windows)]
mod session_close;
#[cfg(windows)]
mod settings_page;
#[cfg(windows)]
mod shelf_dnd;
#[cfg(windows)]
mod shelf_float;
#[cfg(windows)]
mod shelf_refs;
#[cfg(windows)]
mod shelves_page;
#[cfg(windows)]
mod toast;
#[cfg(windows)]
mod todo_float;
#[cfg(windows)]
mod update;
#[cfg(windows)]
mod update_http;
#[cfg(windows)]
mod web_icons;
#[cfg(windows)]
mod winutil;

fn main() {
    // 凭据验证在打开数据目录之前返回。没有这个参数时不碰凭据管理器。
    if let Some(code) = lanwork_toolhost::probe::dispatch_from_args() {
        std::process::exit(code);
    }
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
    fn due_notifications_keep_the_installed_activator_and_skip_portable_click() {
        let toast = include_str!("toast.rs");
        let shortcut = include_str!("../../../tools/shortcut/src/lib.rs");
        assert!(shortcut.contains("0xE8E9_EF84_55BA_4A0C_B4DC_9F9F_4A28_068D"));
        assert!(toast.contains("0xE8E9_EF84_55BA_4A0C_B4DC_9F9F_4A28_068D"));
        assert!(toast.contains("REGCLS_SUSPENDED"));
        assert!(toast.contains("CoResumeClassObjects"));
        assert!(!toast.contains("implement(IAgileObject)"));
        assert!(!toast.contains("RegSetValue"));
        assert!(!toast.contains("CustomActivator"));
        let registry = include_str!("registry.rs");
        assert!(registry.contains("write_portable_display_name"));
        assert!(!registry.contains("CustomActivator"));
        assert!(!registry.contains("LocalServer32"));
        let platform = include_str!("platform.rs");
        assert!(platform.contains("WM_POWERBROADCAST"));
        assert!(platform.contains("WM_TIMECHANGE"));
        assert!(platform.contains("PBT_APMRESUMEAUTOMATIC"));
        let iss = include_str!("../../../packaging/lanwork.iss");
        assert!(iss.contains("CustomActivator"));
        assert!(iss.contains("{E8E9EF84-55BA-4A0C-B4DC-9F9F4A28068D}"));
    }

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
            ("todofloat.slint", include_str!("../ui/todofloat.slint")),
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
            ("todofloat.slint", include_str!("../ui/todofloat.slint")),
            ("float_shell.slint", include_str!("../ui/float_shell.slint")),
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
    fn todo_float_can_check_rows_and_cannot_edit_them() {
        let source = include_str!("../ui/todofloat.slint");
        assert!(source.contains("没有待办"));
        assert!(source.contains("toggle-row"));
        assert!(source.contains("row.overdue ? root.c.critical"));
        assert!(!source.contains("TextInput"), "待办悬浮窗不能改标题");
        assert!(!source.contains("add-item"), "待办悬浮窗不能新建");
        assert!(!source.contains("name: \"新建"));
        assert!(source.contains("limit-width"));
        assert!(source.contains("min(200px, limit-width)"));
        let panel = include_str!("../ui/panel.slint");
        assert!(panel.contains("callback float-todos();"));
        assert!(panel.contains("text: \"悬浮\";"));
        assert!(
            panel.contains("root.float-todos();"),
            "待办页的悬浮按钮要打开窗口"
        );
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

    #[test]
    fn the_float_shell_demo_starts_with_the_app_and_stays_out_of_the_stack() {
        let host = include_str!("host.rs");
        assert!(host.contains("--float-shell-demo"));
        assert!(host.contains("float_shell::schedule_restore"));
        assert!(host.contains("float_shell::schedule_demo"));
        assert!(host.contains("float_shell::save_for_quit"));
        let source = include_str!("float_shell.rs");
        assert!(source.contains("set_skip_taskbar"));
        assert!(source.contains("float_frame::sync"));
        assert!(source.contains("release_topmost"));
        assert!(source.contains("refresh_backdrop"));
        assert!(source.contains("preserve_file"));
        assert!(source.contains("GeometryUnreadable") || source.contains("keep_file_on_open"));
        assert!(source.contains("write_failure_text"));
        assert!(source.contains("quit_in_progress"));
        assert!(source.contains("Timer::single_shot"));
        assert!(!source.contains("float_group"));
        assert!(!source.contains("floatStack"));
        let notes = include_str!("note_float.rs");
        let shelves = include_str!("shelf_float.rs");
        let todos = include_str!("todo_float.rs");
        assert!(!notes.contains("FloatShell"));
        assert!(!shelves.contains("FloatShell"));
        assert!(!todos.contains("FloatShell"));
        assert!(!notes.contains("request_quit"));
        assert!(!shelves.contains("request_quit"));
        assert!(!todos.contains("request_quit"));
        for (name, source) in [("便签", notes), ("收纳", shelves), ("待办", todos)] {
            let close = source
                .split("on_close_requested")
                .nth(1)
                .unwrap_or_else(|| panic!("{name} 没有 WM_CLOSE"));
            let close = close.split("on_strip_clicked").next().unwrap();
            assert!(
                close.contains("quit_in_progress") && close.contains("Timer::single_shot"),
                "{name} 的 WM_CLOSE 要推迟，退出进行中不记成已关"
            );
            let click = source
                .split("on_close_clicked")
                .nth(1)
                .unwrap_or_else(|| panic!("{name} 没有关闭按钮"));
            let click = click.split("on_close_requested").next().unwrap();
            assert!(
                !click.contains("quit_in_progress"),
                "{name} 的关闭按钮仍是马上关"
            );
        }
        let bar = include_str!("bar_win.rs");
        let release = bar
            .split("pub(crate) fn release_topmost")
            .nth(1)
            .expect("release_topmost");
        let release = release.split("pub(crate) fn ").next().unwrap();
        assert!(
            release.contains("float_frame::after_topmost"),
            "取消钉住后要像置顶那样重设边框和磨砂"
        );
        let search = include_str!("searchbar.rs");
        let panel = include_str!("panel.rs");
        assert!(!search.contains("request_quit"));
        assert!(!panel.contains("request_quit"));
        assert!(panel.contains("on_close_panel"));
        let search_close = search
            .split("on_close_requested")
            .nth(1)
            .expect("搜索条关闭");
        assert!(
            search_close.contains("Bar::hide"),
            "搜索条 Alt+F4 只收起，并保留未提交的 /note"
        );
        let panel_close = panel.split("on_close_requested").nth(1).expect("面板关闭");
        assert!(
            panel_close.contains("Controller::hide"),
            "面板 Alt+F4 只收起"
        );
        let session = include_str!("session_close.rs");
        assert!(session.contains("Winit Thread Event Target"));
        assert!(session.contains("request_quit"));
        let query = session
            .split("message == WM_QUERYENDSESSION")
            .nth(1)
            .expect("注销询问");
        let before_sync = query.split("request_quit()").next().unwrap();
        assert!(
            before_sync.contains("Timer::single_shot"),
            "隐藏目标窗口的注销询问要投递退出后马上返回"
        );
        assert!(include_str!("platform.rs").contains("external_close_requests_quit"));
        assert!(host.contains("session_close::install"));
        assert!(host.contains("EXITING"));
    }
}
