//! 把一组热键注册到系统。失败时恢复注册前的那一组。
//!
//! 真正的 `RegisterHotKey` 在外壳。这里只规定顺序：先卸下旧的，再注册新的；
//! 中途失败则卸下已经注册的新热键，再把旧的注册回去。

use crate::config::{Config, ConfigError, Hotkey, parse_hotkey};

/// 搜索条热键的 id。面板热键用 [`PANEL_HOTKEY_ID`]。
pub const SEARCH_HOTKEY_ID: i32 = 1;
pub const PANEL_HOTKEY_ID: i32 = 2;

/// 已经或将要注册的一条热键。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredHotkey {
    pub id: i32,
    pub hotkey: Hotkey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindError {
    /// 系统拒绝注册。调用返回时，端口上的热键应仍是原来的一组。
    Occupied { id: i32 },
}

impl std::fmt::Display for BindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Occupied { .. } => write!(f, "热键已被占用"),
        }
    }
}

/// 操作系统上的热键注册。`register` 返回 false 表示被占用或系统拒绝。
pub trait HotkeyPort {
    fn register(&mut self, id: i32, hotkey: &Hotkey) -> bool;
    fn unregister(&mut self, id: i32);
}

/// 从一份已经 [`crate::config::normalize`] 过的配置得到要注册的热键。
///
/// 面板热键为空时不产生第二条。
pub fn desired_bindings(config: &Config) -> Result<Vec<RegisteredHotkey>, ConfigError> {
    let search = parse_hotkey(&config.search_hotkey)?;
    let panel = match config.panel_hotkey.as_deref() {
        None => None,
        Some(text) => Some(parse_hotkey(text)?),
    };
    if panel == Some(search) {
        return Err(ConfigError::SameHotkey);
    }
    let mut bindings = vec![RegisteredHotkey {
        id: SEARCH_HOTKEY_ID,
        hotkey: search,
    }];
    if let Some(hotkey) = panel {
        bindings.push(RegisteredHotkey {
            id: PANEL_HOTKEY_ID,
            hotkey,
        });
    }
    Ok(bindings)
}

/// 让端口上的热键变成 `next`。内容相同时不调用端口。
pub fn apply_hotkeys<P: HotkeyPort>(
    port: &mut P,
    current: &[RegisteredHotkey],
    next: &[RegisteredHotkey],
) -> Result<(), BindError> {
    if same(current, next) {
        return Ok(());
    }
    for item in current {
        port.unregister(item.id);
    }
    let mut applied: Vec<RegisteredHotkey> = Vec::new();
    for item in next {
        if !port.register(item.id, &item.hotkey) {
            for done in &applied {
                port.unregister(done.id);
            }
            for old in current {
                let _ = port.register(old.id, &old.hotkey);
            }
            return Err(BindError::Occupied { id: item.id });
        }
        applied.push(item.clone());
    }
    Ok(())
}

fn same(current: &[RegisteredHotkey], next: &[RegisteredHotkey]) -> bool {
    current.len() == next.len()
        && current
            .iter()
            .zip(next)
            .all(|(left, right)| left.id == right.id && left.hotkey == right.hotkey)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::collections::BTreeMap;

    struct MapPort {
        live: BTreeMap<i32, Hotkey>,
        fail_key: Option<Hotkey>,
    }

    impl HotkeyPort for MapPort {
        fn register(&mut self, id: i32, hotkey: &Hotkey) -> bool {
            if self.fail_key == Some(*hotkey) {
                return false;
            }
            self.live.insert(id, *hotkey);
            true
        }

        fn unregister(&mut self, id: i32) {
            self.live.remove(&id);
        }
    }

    #[test]
    fn occupied_hotkey_restores_the_previous_registration() {
        let mut config = Config::default();
        let current = desired_bindings(&config).unwrap();
        config.search_hotkey = "Ctrl+Alt+N".into();
        let next = desired_bindings(&config).unwrap();
        let occupied = next[0].hotkey;
        let mut port = MapPort {
            live: BTreeMap::new(),
            fail_key: None,
        };
        apply_hotkeys(&mut port, &[], &current).unwrap();
        port.fail_key = Some(occupied);
        let err = apply_hotkeys(&mut port, &current, &next).unwrap_err();
        assert_eq!(err.to_string(), "热键已被占用");
        assert_eq!(port.live.get(&SEARCH_HOTKEY_ID), Some(&current[0].hotkey));
        assert!(!port.live.contains_key(&PANEL_HOTKEY_ID));
    }

    #[test]
    fn clearing_the_panel_hotkey_unregisters_it() {
        let mut config = Config {
            panel_hotkey: Some("Ctrl+Shift+P".into()),
            ..Config::default()
        };
        let current = desired_bindings(&config).unwrap();
        config.panel_hotkey = None;
        let next = desired_bindings(&config).unwrap();
        let mut port = MapPort {
            live: BTreeMap::new(),
            fail_key: None,
        };
        apply_hotkeys(&mut port, &[], &current).unwrap();
        apply_hotkeys(&mut port, &current, &next).unwrap();
        assert_eq!(port.live.len(), 1);
        assert!(port.live.contains_key(&SEARCH_HOTKEY_ID));
    }
}
