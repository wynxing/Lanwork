//! 配置。
//!
//! 不依赖 Slint，也不调用 Win32。热键注册、主题监听、开机启动的注册表项在 `crates/app`。
//! 字段和缺省见 [`model`]。安静时段的时间格式、GitHub 刷新间隔的含义、
//! 以及若干缺省值，产品规格没有写，不能从这里推断成产品规则。

mod command;
mod error;
mod hotkey;
mod model;
mod service;

pub use command::ConfigCommands;
pub use error::ConfigError;
pub use hotkey::{Hotkey, HotkeyKey, default_search_hotkey, parse_hotkey};
pub use model::{
    Config, DEFAULT_BREAK_MINUTES, DEFAULT_LONG_BREAK_MINUTES, DEFAULT_WORK_MINUTES, HotCorner,
    MAX_BREAK_MINUTES, MAX_WORK_MINUTES, MIN_BREAK_MINUTES, MIN_WORK_MINUTES, Theme, normalize,
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
        assert_eq!(config.stale_days, 14);
        assert_eq!(config.process_snooze_days, 3);
        assert_eq!(config.pomodoro_work_minutes, 25);
        assert_eq!(config.pomodoro_break_minutes, 5);
        assert_eq!(config.pomodoro_long_break_minutes, 15);
        assert_eq!(config.github_refresh_interval_ms, 0);
        assert_eq!(config.web_search_engine, WebSearchEngine::Google);
        assert!(!store.document_path(&DocumentId::Config).unwrap().exists());
    }

    #[test]
    fn roundtrip_and_old_file_without_new_fields() {
        let (_temp, store, commands) = open();
        let mut next = commands.current();
        next.theme = Theme::Dark;
        next.launch_at_startup = true;
        next.stale_days = 0;
        next.panel_hotkey = Some("ctrl+shift+f2".into());
        next.web_search_engine = WebSearchEngine::Baidu;
        commands.replace(next).unwrap();
        let again = ConfigCommands::open(store.clone()).unwrap();
        assert_eq!(again.fallback(), None);
        let config = again.current();
        assert_eq!(config.theme, Theme::Dark);
        assert!(config.launch_at_startup);
        assert_eq!(config.stale_days, 0);
        assert_eq!(config.panel_hotkey.as_deref(), Some("Ctrl+Shift+F2"));
        assert_eq!(config.github_settings().stale_days, 0);
        assert_eq!(config.web_search_engine, WebSearchEngine::Baidu);
        let saved: serde_json::Value = serde_json::from_slice(
            &std::fs::read(store.document_path(&DocumentId::Config).unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(saved["webSearchEngine"], "baidu");
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
    fn quiet_hours_object_is_rejected() {
        let (_temp, _store, commands) = open();
        let mut next = commands.current();
        next.quiet_hours = Some(serde_json::json!({"start": "22:00"}));
        let err = commands.replace(next).unwrap_err();
        assert_eq!(err.to_string(), "安静时段的格式尚未确定");
        assert!(commands.current().quiet_hours.is_none());
    }
}
