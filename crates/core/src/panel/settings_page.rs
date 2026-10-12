//! 面板设置页的决定：热角的选项和顺序、各行显示的值，以及「先写 `config.json`，成功后才生效」的顺序。
//!
//! 不调用窗口、注册表和热键 API，这些在 `crates/app`。这一版只有三项：开机启动、热角、搜索条热键。
//! 热键的注册、写盘和回退走 [`crate::shell::save_search_hotkey`]，不在这里重复。
//! 界面文字只有名称、状态和错误，不写说明（产品规格「全局规则」第 1 条）。

use crate::config::{Config, ConfigCommands, ConfigError, HotCorner};

/// 热角选项，顺序与产品规格「面板」一致：右上（默认）、左上、左下、右下、关闭。
pub const HOT_CORNER_CHOICES: [HotCorner; 5] = [
    HotCorner::TopRight,
    HotCorner::TopLeft,
    HotCorner::BottomLeft,
    HotCorner::BottomRight,
    HotCorner::Off,
];

/// 设置页各行的名称。
pub const LAUNCH_AT_STARTUP_TITLE: &str = "开机启动";
pub const HOT_CORNER_TITLE: &str = "热角";
pub const SEARCH_HOTKEY_TITLE: &str = "搜索条热键";
/// 热键框在录入时显示的状态。
pub const RECORDING_STATE: &str = "录入中";

#[must_use]
pub fn hot_corner_title(corner: HotCorner) -> &'static str {
    match corner {
        HotCorner::TopRight => "右上",
        HotCorner::TopLeft => "左上",
        HotCorner::BottomLeft => "左下",
        HotCorner::BottomRight => "右下",
        HotCorner::Off => "关闭",
    }
}

#[must_use]
pub fn hot_corner_titles() -> [&'static str; 5] {
    HOT_CORNER_CHOICES.map(hot_corner_title)
}

#[must_use]
pub fn hot_corner_index(corner: HotCorner) -> usize {
    HOT_CORNER_CHOICES
        .iter()
        .position(|choice| *choice == corner)
        .unwrap_or(0)
}

#[must_use]
pub fn hot_corner_at(index: i32) -> Option<HotCorner> {
    usize::try_from(index)
        .ok()
        .and_then(|index| HOT_CORNER_CHOICES.get(index).copied())
}

/// 设置页要显示的值，全部来自配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsRows {
    pub launch_at_startup: bool,
    pub hot_corner: usize,
    pub search_hotkey: String,
}

#[must_use]
pub fn settings_rows(config: &Config) -> SettingsRows {
    SettingsRows {
        launch_at_startup: config.launch_at_startup,
        hot_corner: hot_corner_index(config.hot_corner),
        search_hotkey: config.search_hotkey.clone(),
    }
}

/// 开机启动写进配置之后的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupChange {
    /// 配置里现在的值，界面显示它。
    pub enabled: bool,
    /// 注册表没写成时的原因。配置已经写下，不改回去：下一次启动仍按配置再同步一次。
    pub registry_error: Option<String>,
}

/// 开机启动开关：先写 `config.json`，写成了才调用 `apply` 去写或删 Run 值。
///
/// 写盘失败时不调用 `apply`，返回错误，界面保持原值。`apply` 失败不回滚配置。
pub fn change_launch_at_startup(
    commands: &ConfigCommands,
    enabled: bool,
    apply: impl FnOnce(bool) -> Result<(), String>,
) -> Result<StartupChange, ConfigError> {
    let saved = commands.set_launch_at_startup(enabled)?;
    let registry_error = apply(saved.launch_at_startup).err();
    Ok(StartupChange {
        enabled: saved.launch_at_startup,
        registry_error,
    })
}

/// 热角：先写 `config.json`，写成了才调用 `apply` 让轮询立即换角。写盘失败时不调用 `apply`。
pub fn change_hot_corner(
    commands: &ConfigCommands,
    corner: HotCorner,
    apply: impl FnOnce(HotCorner),
) -> Result<HotCorner, ConfigError> {
    let saved = commands.set_hot_corner(corner)?;
    apply(saved.hot_corner);
    Ok(saved.hot_corner)
}

/// 注册表写不成时显示的错误。
#[must_use]
pub fn startup_error_text(reason: &str) -> String {
    format!("开机启动项未更新：{reason}")
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::config::Theme;
    use crate::storage::{DocumentId, Store, StorePaths};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "lanwork-settings-{nanos}-{}",
                SEQ.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn open() -> (TempDir, Store, ConfigCommands) {
        let temp = TempDir::new();
        let store = Store::open(StorePaths {
            data_dir: temp.0.join("data"),
            cache_dir: temp.0.join("cache"),
            user_profile: temp.0.join("profile"),
            local_app_data: temp.0.join("local"),
        })
        .unwrap();
        let commands = ConfigCommands::open(store.clone()).unwrap();
        (temp, store, commands)
    }

    #[test]
    fn corner_options_follow_the_product_order_and_default_first() {
        assert_eq!(
            hot_corner_titles(),
            ["右上", "左上", "左下", "右下", "关闭"]
        );
        assert_eq!(HOT_CORNER_CHOICES[0], HotCorner::default());
        for (index, corner) in HOT_CORNER_CHOICES.iter().enumerate() {
            assert_eq!(hot_corner_index(*corner), index);
            assert_eq!(hot_corner_at(i32::try_from(index).unwrap()), Some(*corner));
        }
        assert_eq!(hot_corner_at(-1), None);
        assert_eq!(hot_corner_at(5), None);
    }

    #[test]
    fn rows_show_the_configured_values() {
        let rows = settings_rows(&Config::default());
        assert_eq!(
            rows,
            SettingsRows {
                launch_at_startup: false,
                hot_corner: 0,
                search_hotkey: "Ctrl+Alt+M".into(),
            }
        );
        let rows = settings_rows(&Config {
            launch_at_startup: true,
            hot_corner: HotCorner::Off,
            search_hotkey: "Ctrl+Alt+K".into(),
            ..Config::default()
        });
        assert_eq!(rows.hot_corner, 4);
        assert!(rows.launch_at_startup);
        assert_eq!(rows.search_hotkey, "Ctrl+Alt+K");
    }

    #[test]
    fn startup_is_written_to_config_before_the_registry_is_touched() {
        let (_temp, store, commands) = open();
        let seen_in_file = RefCell::new(None);
        let path = store.document_path(&DocumentId::Config).unwrap();
        let change = change_launch_at_startup(&commands, true, |enabled| {
            let saved: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            *seen_in_file.borrow_mut() = Some((enabled, saved["launchAtStartup"].clone()));
            Ok(())
        })
        .unwrap();
        assert_eq!(
            change,
            StartupChange {
                enabled: true,
                registry_error: None
            }
        );
        assert_eq!(
            seen_in_file.into_inner(),
            Some((true, serde_json::Value::Bool(true)))
        );

        let off = change_launch_at_startup(&commands, false, |enabled| {
            assert!(!enabled);
            Ok(())
        })
        .unwrap();
        assert!(!off.enabled);
        assert!(
            !ConfigCommands::open(store)
                .unwrap()
                .current()
                .launch_at_startup
        );
    }

    #[test]
    fn a_registry_failure_keeps_the_config_value_and_reports_the_reason() {
        let (_temp, store, commands) = open();
        let change =
            change_launch_at_startup(&commands, true, |_| Err("注册表写入失败".into())).unwrap();
        assert!(change.enabled);
        assert_eq!(change.registry_error.as_deref(), Some("注册表写入失败"));
        assert!(commands.current().launch_at_startup);
        assert!(
            ConfigCommands::open(store)
                .unwrap()
                .current()
                .launch_at_startup
        );
        assert_eq!(
            startup_error_text("注册表写入失败"),
            "开机启动项未更新：注册表写入失败"
        );
    }

    #[test]
    fn a_failed_config_write_never_touches_the_registry_or_the_corner() {
        let (_temp, store, commands) = open();
        let mut start = commands.current();
        start.theme = Theme::Dark;
        commands.replace(start).unwrap();
        let path = store.document_path(&DocumentId::Config).unwrap();
        let before = std::fs::read(&path).unwrap();
        std::fs::create_dir(path.with_file_name("config.json.tmp")).unwrap();

        let touched = Cell::new(false);
        let err = change_launch_at_startup(&commands, true, |_| {
            touched.set(true);
            Ok(())
        })
        .unwrap_err();
        assert!(matches!(err, ConfigError::Store(_)), "{err}");
        let err =
            change_hot_corner(&commands, HotCorner::BottomLeft, |_| touched.set(true)).unwrap_err();
        assert!(matches!(err, ConfigError::Store(_)), "{err}");

        assert!(!touched.get());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let rows = settings_rows(&commands.current());
        assert!(!rows.launch_at_startup);
        assert_eq!(rows.hot_corner, 0);
    }

    #[test]
    fn every_corner_is_saved_then_applied_and_survives_a_restart() {
        let (_temp, store, commands) = open();
        let applied = RefCell::new(Vec::new());
        for corner in [
            HotCorner::BottomRight,
            HotCorner::Off,
            HotCorner::TopLeft,
            HotCorner::BottomLeft,
            HotCorner::TopRight,
        ] {
            let saved =
                change_hot_corner(&commands, corner, |c| applied.borrow_mut().push(c)).unwrap();
            assert_eq!(saved, corner);
            assert_eq!(
                ConfigCommands::open(store.clone())
                    .unwrap()
                    .current()
                    .hot_corner,
                corner
            );
        }
        assert_eq!(
            applied.into_inner(),
            [
                HotCorner::BottomRight,
                HotCorner::Off,
                HotCorner::TopLeft,
                HotCorner::BottomLeft,
                HotCorner::TopRight,
            ]
        );
    }
}
