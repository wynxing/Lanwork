//! 配置。
//!
//! 不依赖 Slint，也不调用 Win32。热键注册、主题监听、开机启动的注册表项在 `crates/app`。
//! 字段和缺省见 [`model`]。安静时段的时间格式，以及若干缺省值，产品规格没有写，
//! 不能从这里推断成产品规则。旧文件里的 `githubRefreshIntervalMs`、`staleDays`、
//! `sourceSync`、`autoCompleteOnClose` 在读取时忽略，写回时不再写出。

mod command;
mod error;
mod hotkey;
mod model;
mod quiet;
mod service;

pub use command::ConfigCommands;
pub use error::ConfigError;
pub use hotkey::{Hotkey, HotkeyKey, default_search_hotkey, parse_hotkey};
pub use model::{
    Config, DEFAULT_BREAK_MINUTES, DEFAULT_LONG_BREAK_MINUTES, DEFAULT_WORK_MINUTES, HotCorner,
    MAX_BREAK_MINUTES, MAX_WORK_MINUTES, MIN_BREAK_MINUTES, MIN_WORK_MINUTES, SNOOZE_DAYS_ERROR,
    SearchBarOrigin, Theme, normalize,
};
pub use quiet::{
    CLOCK_FORMAT_ERROR, QuietHours, default_quiet_hours, format_clock, is_quiet_now, parse_clock,
};
pub use service::{ConfigService, Fallback};

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::search::WebSearchEngine;
    use crate::storage::{DocumentId, EntityKind, Store, StorePaths};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("lanwork-config-{nanos}-{seq}"));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn open() -> (TempDir, Store, ConfigCommands) {
        let temp = TempDir::new();
        let store = Store::open(StorePaths {
            data_dir: temp.path.join("data"),
            cache_dir: temp.path.join("cache"),
            user_profile: temp.path.join("profile"),
            local_app_data: temp.path.join("local"),
        })
        .unwrap();
        let commands = ConfigCommands::open(store.clone()).unwrap();
        (temp, store, commands)
    }

    #[test]
    fn missing_file_uses_defaults_and_does_not_write() {
        let (_temp, store, commands) = open();
        assert_eq!(commands.fallback(), Some(Fallback::Missing));
        let config = commands.current();
        assert_eq!(config.search_hotkey, "Ctrl+Alt+M");
        assert_eq!(config.panel_hotkey, None);
        assert_eq!(config.hot_corner, HotCorner::TopRight);
        assert_eq!(config.theme, Theme::System);
        assert!(!config.launch_at_startup);
        assert_eq!(config.process_snooze_days, 3);
        assert_eq!(config.pomodoro_work_minutes, 25);
        assert_eq!(config.pomodoro_break_minutes, 5);
        assert_eq!(config.pomodoro_long_break_minutes, 15);
        assert_eq!(config.web_search_engine, WebSearchEngine::Google);
        assert!(config.search_bar_origin.is_none());
        assert!(!store.document_path(&DocumentId::Config).unwrap().exists());
    }

    #[test]
    fn roundtrip_and_old_file_without_new_fields() {
        let (_temp, store, commands) = open();
        let mut next = commands.current();
        next.theme = Theme::Dark;
        next.launch_at_startup = true;
        next.panel_hotkey = Some("ctrl+shift+f2".into());
        next.web_search_engine = WebSearchEngine::Baidu;
        commands.replace(next).unwrap();
        let again = ConfigCommands::open(store.clone()).unwrap();
        assert_eq!(again.fallback(), None);
        let config = again.current();
        assert_eq!(config.theme, Theme::Dark);
        assert!(config.launch_at_startup);
        assert_eq!(config.panel_hotkey.as_deref(), Some("Ctrl+Shift+F2"));
        assert_eq!(config.web_search_engine, WebSearchEngine::Baidu);
        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(store.document_path(&DocumentId::Config).unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["webSearchEngine"], "baidu");
        assert!(saved.get("searchBarOrigin").is_none());
    }

    #[test]
    fn incomplete_search_bar_origin_keeps_the_file_and_uses_defaults() {
        let (_temp, store, _) = open();
        let path = store.document_path(&DocumentId::Config).unwrap();
        let original = br#"{"schemaVersion":1,"searchBarOrigin":{"x":1},"theme":"dark"}"#;
        std::fs::write(&path, original).unwrap();
        let commands = ConfigCommands::open(store).unwrap();
        assert_eq!(commands.fallback(), Some(Fallback::Invalid));
        assert!(commands.current().search_bar_origin.is_none());
        assert_eq!(commands.current().theme, Theme::System);
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn search_bar_origin_is_absent_until_set_and_roundtrips() {
        let (_temp, store, _) = open();
        let path = store.document_path(&DocumentId::Config).unwrap();
        std::fs::write(
            &path,
            br#"{"schemaVersion":1,"searchHotkey":"Ctrl+Alt+M","theme":"dark"}"#,
        )
        .unwrap();
        let commands = ConfigCommands::open(store.clone()).unwrap();
        assert_eq!(commands.fallback(), None);
        assert_eq!(commands.current().theme, Theme::Dark);
        assert!(commands.current().search_bar_origin.is_none());

        let mut next = commands.current();
        next.search_bar_origin = Some(SearchBarOrigin { x: -40, y: 120 });
        commands.replace(next).unwrap();
        let again = ConfigCommands::open(store.clone()).unwrap();
        assert_eq!(
            again.current().search_bar_origin,
            Some(SearchBarOrigin { x: -40, y: 120 })
        );
        assert_eq!(again.current().theme, Theme::Dark);
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["searchBarOrigin"]["x"], -40);
        assert_eq!(saved["searchBarOrigin"]["y"], 120);
    }

    #[test]
    fn invalid_hotkey_and_same_hotkey_do_not_replace() {
        let (_temp, store, commands) = open();
        let mut saved = commands.current();
        saved.theme = Theme::Light;
        commands.replace(saved).unwrap();
        let before = std::fs::read(store.document_path(&DocumentId::Config).unwrap()).unwrap();

        let err = commands
            .replace(Config {
                search_hotkey: "Ctrl+Nope".into(),
                ..commands.current()
            })
            .unwrap_err();
        assert_eq!(err.to_string(), "热键无效");
        let err = commands
            .replace(Config {
                panel_hotkey: Some("alt+ctrl+m".into()),
                ..commands.current()
            })
            .unwrap_err();
        assert_eq!(err.to_string(), "搜索条热键与面板热键相同");
        let err = commands
            .replace(Config {
                search_hotkey: "   ".into(),
                ..commands.current()
            })
            .unwrap_err();
        assert_eq!(err.to_string(), "搜索条热键不能为空");

        let bytes = std::fs::read(store.document_path(&DocumentId::Config).unwrap()).unwrap();
        assert_eq!(bytes, before);
        assert_eq!(commands.current().theme, Theme::Light);
        assert_eq!(commands.current().search_hotkey, "Ctrl+Alt+M");
    }

    #[test]
    fn old_legal_hotkeys_load_and_bare_space_does_not() {
        let (_temp, store, _) = open();
        let path = store.document_path(&DocumentId::Config).unwrap();
        std::fs::write(
            &path,
            br#"{"schemaVersion":1,"searchHotkey":"Ctrl+Alt+M","panelHotkey":"F12"}"#,
        )
        .unwrap();
        let commands = ConfigCommands::open(store.clone()).unwrap();
        assert_eq!(commands.fallback(), None);
        assert_eq!(commands.current().search_hotkey, "Ctrl+Alt+M");
        assert_eq!(commands.current().panel_hotkey.as_deref(), Some("F12"));

        std::fs::write(
            &path,
            br#"{"schemaVersion":1,"searchHotkey":"ctrl+shift+f2","panelHotkey":"alt + 7"}"#,
        )
        .unwrap();
        let commands = ConfigCommands::open(store.clone()).unwrap();
        assert_eq!(commands.fallback(), None);
        assert_eq!(commands.current().search_hotkey, "Ctrl+Shift+F2");
        assert_eq!(commands.current().panel_hotkey.as_deref(), Some("Alt+7"));

        let mut next = commands.current();
        next.search_hotkey = "Ctrl+Space".into();
        next.panel_hotkey = Some("Alt+Left".into());
        commands.replace(next).unwrap();
        let mut next = commands.current();
        next.search_hotkey = "Ctrl+Shift+/".into();
        next.panel_hotkey = Some("Ctrl+Numpad1".into());
        commands.replace(next).unwrap();
        let again = ConfigCommands::open(store.clone()).unwrap();
        assert_eq!(again.fallback(), None);
        assert_eq!(again.current().search_hotkey, "Ctrl+Shift+/");
        assert_eq!(
            again.current().panel_hotkey.as_deref(),
            Some("Ctrl+Numpad1")
        );
        drop(again);
        drop(commands);

        let original = br#"{"schemaVersion":1,"searchHotkey":"Space"}"#;
        std::fs::write(&path, original).unwrap();
        let commands = ConfigCommands::open(store).unwrap();
        assert_eq!(commands.fallback(), Some(Fallback::Invalid));
        assert_eq!(commands.current().search_hotkey, "Ctrl+Alt+M");
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn ranges_reject_without_writing() {
        let (_temp, store, commands) = open();
        commands.replace(commands.current()).unwrap();
        let path = store.document_path(&DocumentId::Config).unwrap();
        let before = std::fs::read(&path).unwrap();
        for (field, message) in [
            ("process_snooze_days", "处理模式顺延天数只接受 1 至 30"),
            ("pomodoro_work_minutes", "工作时长只接受 1 至 120 分钟"),
            ("pomodoro_break_minutes", "休息时长只接受 1 至 60 分钟"),
            (
                "pomodoro_long_break_minutes",
                "长休息时长只接受 1 至 60 分钟",
            ),
        ] {
            let mut next = commands.current();
            match field {
                "process_snooze_days" => next.process_snooze_days = 0,
                "pomodoro_work_minutes" => next.pomodoro_work_minutes = 121,
                "pomodoro_break_minutes" => next.pomodoro_break_minutes = 0,
                "pomodoro_long_break_minutes" => next.pomodoro_long_break_minutes = 61,
                _ => unreachable!(),
            }
            let err = commands.replace(next).unwrap_err();
            assert_eq!(err.to_string(), message);
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn corrupt_json_is_quarantined_and_starts_from_defaults() {
        let (_temp, store, _) = open();
        let path = store.document_path(&DocumentId::Config).unwrap();
        std::fs::write(&path, b"{").unwrap();
        let commands = ConfigCommands::open(store).unwrap();
        assert_eq!(commands.fallback(), Some(Fallback::Quarantined));
        assert_eq!(commands.current().search_hotkey, "Ctrl+Alt+M");
        assert!(!path.exists());
        let parent = path.parent().unwrap();
        let quarantined = std::fs::read_dir(parent).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("config.json.corrupt-")
        });
        assert!(quarantined);
    }

    #[test]
    fn invalid_fields_keep_the_file_and_use_defaults_in_memory() {
        let (_temp, store, _) = open();
        let path = store.document_path(&DocumentId::Config).unwrap();
        std::fs::write(
            &path,
            br#"{"schemaVersion":1,"searchHotkey":"nope","theme":"dark"}"#,
        )
        .unwrap();
        let commands = ConfigCommands::open(store).unwrap();
        assert_eq!(commands.fallback(), Some(Fallback::Invalid));
        assert_eq!(commands.current().theme, Theme::System);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            br#"{"schemaVersion":1,"searchHotkey":"nope","theme":"dark"}"#
        );
    }

    #[test]
    fn unsupported_schema_is_not_quarantined_or_replaced() {
        let (_temp, store, _) = open();
        let path = store.document_path(&DocumentId::Config).unwrap();
        let original = br#"{"schemaVersion":99,"searchHotkey":"Ctrl+Alt+M"}"#;
        std::fs::write(&path, original).unwrap();
        let err = ConfigCommands::open(store).unwrap_err();
        assert!(err.to_string().contains("99"));
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn unknown_theme_keeps_the_file() {
        let (_temp, store, _) = open();
        let path = store.document_path(&DocumentId::Config).unwrap();
        let original = br#"{"schemaVersion":1,"theme":"blue","searchHotkey":"Ctrl+Alt+M"}"#;
        std::fs::write(&path, original).unwrap();
        let commands = ConfigCommands::open(store).unwrap();
        assert_eq!(commands.fallback(), Some(Fallback::Invalid));
        assert_eq!(commands.current().theme, Theme::System);
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn unknown_web_search_engine_keeps_the_file() {
        let (_temp, store, _) = open();
        let path = store.document_path(&DocumentId::Config).unwrap();
        let original = br#"{"schemaVersion":1,"webSearchEngine":"yahoo"}"#;
        std::fs::write(&path, original).unwrap();
        let commands = ConfigCommands::open(store).unwrap();
        assert_eq!(commands.fallback(), Some(Fallback::Invalid));
        assert_eq!(
            commands.current().web_search_engine,
            WebSearchEngine::Google
        );
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn change_is_published_only_after_the_memory_callback() {
        let (_temp, store, commands) = open();
        let events = store.subscribe();
        let next = commands.current();
        let mut visible_during_callback = false;
        store
            .write_json_with_before_publish(&DocumentId::Config, &next, || {
                visible_during_callback = events.try_recv().is_ok();
            })
            .unwrap();
        assert!(!visible_during_callback);
        assert_eq!(events.try_recv().unwrap().kind, EntityKind::Config);
    }

    #[test]
    fn replace_has_the_new_config_when_the_change_is_visible() {
        let (_temp, store, commands) = open();
        let events = store.subscribe();
        let mut next = commands.current();
        next.search_hotkey = "Ctrl+Alt+N".into();
        commands.replace(next).unwrap();
        assert_eq!(events.try_recv().unwrap().kind, EntityKind::Config);
        assert_eq!(commands.current().search_hotkey, "Ctrl+Alt+N");
    }

    #[test]
    fn removed_github_fields_are_ignored_and_omitted_on_write() {
        let (_temp, store, _) = open();
        let path = store.document_path(&DocumentId::Config).unwrap();
        let original = br#"{"schemaVersion":1,"theme":"dark","searchHotkey":"Ctrl+Alt+M","githubRefreshIntervalMs":"soon","staleDays":false,"sourceSync":1,"autoCompleteOnClose":"yes","processSnoozeDays":4}"#;
        std::fs::write(&path, original).unwrap();
        let commands = ConfigCommands::open(store.clone()).unwrap();
        assert_eq!(commands.fallback(), None);
        let config = commands.current();
        assert_eq!(config.theme, Theme::Dark);
        assert_eq!(config.search_hotkey, "Ctrl+Alt+M");
        assert_eq!(config.process_snooze_days, 4);
        assert_eq!(std::fs::read(&path).unwrap(), original);

        let mut next = commands.current();
        next.launch_at_startup = true;
        commands.replace(next).unwrap();
        assert!(path.exists());
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["theme"], "dark");
        assert_eq!(saved["processSnoozeDays"], 4);
        assert_eq!(saved["launchAtStartup"], true);
        for key in [
            "githubRefreshIntervalMs",
            "staleDays",
            "sourceSync",
            "autoCompleteOnClose",
        ] {
            assert!(saved.get(key).is_none(), "{key} was written back");
        }
        let parent = path.parent().unwrap();
        let quarantined = std::fs::read_dir(parent).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains("config.json.corrupt-")
        });
        assert!(!quarantined);
    }

    #[test]
    fn launch_at_startup_and_hot_corner_persist_across_reopen() {
        let (_temp, store, commands) = open();
        let saved = commands.set_launch_at_startup(true).unwrap();
        assert!(saved.launch_at_startup);
        commands.set_hot_corner(HotCorner::BottomLeft).unwrap();
        for corner in [
            HotCorner::TopLeft,
            HotCorner::BottomRight,
            HotCorner::Off,
            HotCorner::TopRight,
        ] {
            commands.set_hot_corner(corner).unwrap();
            let again = ConfigCommands::open(store.clone()).unwrap();
            assert_eq!(again.current().hot_corner, corner);
            assert!(again.current().launch_at_startup);
        }
        let path = store.document_path(&DocumentId::Config).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(saved["launchAtStartup"], true);
        assert_eq!(saved["hotCorner"], "topRight");
        assert_eq!(saved["searchHotkey"], "Ctrl+Alt+M");

        commands.set_launch_at_startup(false).unwrap();
        let again = ConfigCommands::open(store).unwrap();
        assert!(!again.current().launch_at_startup);
    }

    #[test]
    fn unchanged_value_does_not_write_or_publish() {
        let (_temp, store, commands) = open();
        let path = store.document_path(&DocumentId::Config).unwrap();
        let events = store.subscribe();
        commands.set_launch_at_startup(false).unwrap();
        commands.set_hot_corner(HotCorner::TopRight).unwrap();
        assert!(!path.exists());
        assert!(events.try_recv().is_err());

        commands.set_hot_corner(HotCorner::Off).unwrap();
        assert_eq!(events.try_recv().unwrap().kind, EntityKind::Config);
        let before = std::fs::read(&path).unwrap();
        commands.set_hot_corner(HotCorner::Off).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn a_failed_write_keeps_memory_file_and_other_fields() {
        let (_temp, store, commands) = open();
        let mut start = commands.current();
        start.theme = Theme::Dark;
        start.search_hotkey = "Ctrl+Alt+N".into();
        commands.replace(start).unwrap();
        let path = store.document_path(&DocumentId::Config).unwrap();
        let before = std::fs::read(&path).unwrap();
        let blocker = path.with_file_name("config.json.tmp");
        std::fs::create_dir(&blocker).unwrap();
        let events = store.subscribe();

        let err = commands.set_launch_at_startup(true).unwrap_err();
        assert!(matches!(err, ConfigError::Store(_)), "{err}");
        let err = commands.set_hot_corner(HotCorner::Off).unwrap_err();
        assert!(matches!(err, ConfigError::Store(_)), "{err}");

        let config = commands.current();
        assert!(!config.launch_at_startup);
        assert_eq!(config.hot_corner, HotCorner::TopRight);
        assert_eq!(config.theme, Theme::Dark);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(events.try_recv().is_err());

        std::fs::remove_dir(&blocker).unwrap();
        commands.set_hot_corner(HotCorner::Off).unwrap();
        let again = ConfigCommands::open(store).unwrap();
        assert_eq!(again.current().hot_corner, HotCorner::Off);
        assert_eq!(again.current().search_hotkey, "Ctrl+Alt+N");
    }

    #[test]
    fn quiet_hours_roundtrip_across_midnight_and_same_endpoints_stay_on_disk() {
        let (_temp, store, commands) = open();
        let mut next = commands.current();
        next.quiet_hours = Some(default_quiet_hours());
        next.theme = Theme::Dark;
        commands.replace(next).unwrap();
        let path = store.document_path(&DocumentId::Config).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["quietHours"]["start"], "22:00");
        assert_eq!(saved["quietHours"]["end"], "08:00");
        let again = ConfigCommands::open(store.clone()).unwrap();
        let hours = again.current().quiet_hours.unwrap();
        assert!(is_quiet_now(Some(hours), parse_clock("23:30").unwrap()));
        assert!(is_quiet_now(Some(hours), parse_clock("07:59").unwrap()));
        assert!(!is_quiet_now(Some(hours), parse_clock("08:00").unwrap()));
        assert!(!is_quiet_now(Some(hours), parse_clock("21:59").unwrap()));
        assert_eq!(again.current().theme, Theme::Dark);

        let original = std::fs::read(&path).unwrap();
        let mut same = again.current();
        same.quiet_hours = Some(QuietHours {
            start: parse_clock("10:00").unwrap(),
            end: parse_clock("10:00").unwrap(),
        });
        let err = commands.replace(same).unwrap_err();
        assert_eq!(err.to_string(), "安静时段起止相同");
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert_eq!(commands.current().quiet_hours, Some(default_quiet_hours()));

        commands.set_quiet_hours(None).unwrap();
        let cleared: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(cleared.get("quietHours").is_none());
        assert!(commands.current().quiet_hours.is_none());
        assert!(!is_quiet_now(
            commands.current().quiet_hours,
            parse_clock("23:00").unwrap()
        ));
    }

    #[test]
    fn malformed_quiet_hours_keep_the_file() {
        let (_temp, store, _) = open();
        let path = store.document_path(&DocumentId::Config).unwrap();
        let original = br#"{"schemaVersion":1,"quietHours":{"start":"22:00"},"theme":"dark"}"#;
        std::fs::write(&path, original).unwrap();
        let commands = ConfigCommands::open(store).unwrap();
        assert_eq!(commands.fallback(), Some(Fallback::Invalid));
        assert!(commands.current().quiet_hours.is_none());
        assert_eq!(commands.current().theme, Theme::System);
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[test]
    fn one_digit_quiet_hours_load_and_snooze_setter_rejects_the_ends() {
        let (_temp, store, _) = open();
        let path = store.document_path(&DocumentId::Config).unwrap();
        std::fs::write(
            &path,
            br#"{"schemaVersion":1,"quietHours":{"start":"9:00","end":"17:00"},"processSnoozeDays":3}"#,
        )
        .unwrap();
        let commands = ConfigCommands::open(store).unwrap();
        assert_eq!(commands.fallback(), None);
        let hours = commands.current().quiet_hours.unwrap();
        assert!(is_quiet_now(Some(hours), parse_clock("09:00").unwrap()));
        assert!(!is_quiet_now(Some(hours), parse_clock("17:00").unwrap()));
        assert!(commands.set_process_snooze_days(30).is_ok());
        assert_eq!(commands.current().process_snooze_days, 30);
        let err = commands.set_process_snooze_days(31).unwrap_err();
        assert_eq!(err.to_string(), SNOOZE_DAYS_ERROR);
        assert_eq!(commands.current().process_snooze_days, 30);
        let err = commands.set_process_snooze_days(0).unwrap_err();
        assert_eq!(err.to_string(), SNOOZE_DAYS_ERROR);
    }
}
