//! 把一组热键注册到系统。
//!
//! 真正的 `RegisterHotKey` 在外壳。运行中修改只走先注册、成功后写盘、再卸旧。
//! 本进程里与新组合冲突的注册要先卸掉再注册；失败则全部恢复。注册失败不写配置。

use crate::config::{Config, ConfigCommands, ConfigError, Hotkey, normalize, parse_hotkey};

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

/// 让端口上的热键变成 `next`。先注册新的，成功后再卸掉不再使用的旧 id。
///
/// 新组合若已被本进程的另一条热键占用，先卸掉那条再注册。内容相同时不调用端口。
/// 注册失败时恢复这次动过的热键。
pub fn apply_hotkeys<P: HotkeyPort>(
    port: &mut P,
    current: &[RegisteredHotkey],
    next: &[RegisteredHotkey],
) -> Result<(), BindError> {
    let prepared = prepare(port, current, next)?;
    retire(port, &prepared);
    Ok(())
}

/// 修改热键失败。注册失败时配置没有写入。
#[derive(Debug)]
pub enum SaveHotkeyError {
    Bind(BindError),
    Config(ConfigError),
}

impl std::fmt::Display for SaveHotkeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bind(err) => write!(f, "{err}"),
            Self::Config(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SaveHotkeyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bind(_) => None,
            Self::Config(err) => Some(err),
        }
    }
}

/// 运行中修改热键的入口。先注册 `search` 和 `panel`，成功后再 [`ConfigCommands::replace`]，然后卸掉旧热键。
///
/// 字符串不合法或注册失败时，端口和 `config.json` 都保持原样。
/// 本进程内两条热键互换，或把一条的组合改给另一条时，先卸掉冲突的注册再注册。
pub fn save_hotkeys<P: HotkeyPort>(
    port: &mut P,
    commands: &ConfigCommands,
    current: &[RegisteredHotkey],
    search: &str,
    panel: Option<&str>,
) -> Result<Vec<RegisteredHotkey>, SaveHotkeyError> {
    let mut next = commands.current();
    next.search_hotkey = search.to_owned();
    next.panel_hotkey = panel.map(str::to_owned);
    let next = normalize(next).map_err(SaveHotkeyError::Config)?;
    let bindings = desired_bindings(&next).map_err(SaveHotkeyError::Config)?;
    commit_hotkeys(port, current, &bindings, || {
        commands.replace(next).map(|_| ())
    })
    .map_err(|err| match err {
        CommitError::Occupied(err) => SaveHotkeyError::Bind(err),
        CommitError::Persist(err) => SaveHotkeyError::Config(err),
    })
}

#[derive(Debug)]
enum CommitError<E> {
    Occupied(BindError),
    Persist(E),
}

fn commit_hotkeys<P, E>(
    port: &mut P,
    current: &[RegisteredHotkey],
    next: &[RegisteredHotkey],
    persist: impl FnOnce() -> Result<(), E>,
) -> Result<Vec<RegisteredHotkey>, CommitError<E>>
where
    P: HotkeyPort,
{
    let prepared = prepare(port, current, next).map_err(CommitError::Occupied)?;
    if prepared.undo.is_empty() && prepared.retired.is_empty() {
        return Ok(prepared.next);
    }
    if let Err(err) = persist() {
        undo_hotkeys(port, &prepared.undo);
        return Err(CommitError::Persist(err));
    }
    retire(port, &prepared);
    Ok(prepared.next)
}

enum Undo {
    Restore(RegisteredHotkey),
    DropNew(i32),
}

struct Prepared {
    next: Vec<RegisteredHotkey>,
    undo: Vec<Undo>,
    retired: Vec<i32>,
}

fn prepare<P: HotkeyPort>(
    port: &mut P,
    current: &[RegisteredHotkey],
    next: &[RegisteredHotkey],
) -> Result<Prepared, BindError> {
    if same(current, next) {
        return Ok(Prepared {
            next: next.to_vec(),
            undo: Vec::new(),
            retired: Vec::new(),
        });
    }
    let mut undo = Vec::new();
    let mut live = current.to_vec();
    for item in next {
        if live
            .iter()
            .any(|old| old.id == item.id && old.hotkey == item.hotkey)
        {
            continue;
        }
        let conflicts: Vec<RegisteredHotkey> = live
            .iter()
            .filter(|old| old.id != item.id && old.hotkey == item.hotkey)
            .cloned()
            .collect();
        for conflict in conflicts {
            port.unregister(conflict.id);
            undo.push(Undo::Restore(conflict.clone()));
            live.retain(|old| old.id != conflict.id);
        }
        if !port.register(item.id, &item.hotkey) {
            undo_hotkeys(port, &undo);
            return Err(BindError::Occupied { id: item.id });
        }
        if let Some(old) = live.iter().find(|old| old.id == item.id).cloned() {
            undo.push(Undo::Restore(old));
        } else {
            undo.push(Undo::DropNew(item.id));
        }
        live.retain(|old| old.id != item.id);
        live.push(item.clone());
    }
    let retired = current
        .iter()
        .filter(|old| !next.iter().any(|item| item.id == old.id))
        .map(|old| old.id)
        .collect();
    Ok(Prepared {
        next: next.to_vec(),
        undo,
        retired,
    })
}

fn undo_hotkeys<P: HotkeyPort>(port: &mut P, undo: &[Undo]) {
    for step in undo.iter().rev() {
        match step {
            Undo::Restore(old) => {
                let _ = port.register(old.id, &old.hotkey);
            }
            Undo::DropNew(id) => port.unregister(*id),
        }
    }
}

fn retire<P: HotkeyPort>(port: &mut P, prepared: &Prepared) {
    for id in &prepared.retired {
        port.unregister(*id);
    }
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
    use super::{CommitError, *};
    use crate::config::Config;
    use std::collections::BTreeMap;

    struct MapPort {
        live: BTreeMap<i32, Hotkey>,
        fail_key: Option<Hotkey>,
    }

    impl HotkeyPort for MapPort {
        fn register(&mut self, id: i32, hotkey: &Hotkey) -> bool {
            let taken = self
                .live
                .iter()
                .any(|(other, key)| *other != id && key == hotkey);
            if taken || self.fail_key == Some(*hotkey) {
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

    #[derive(Debug, PartialEq, Eq)]
    enum Op {
        Register(i32),
        Unregister(i32),
        Persist,
    }

    struct LogPort<'a> {
        live: &'a std::cell::RefCell<BTreeMap<i32, Hotkey>>,
        ops: &'a std::cell::RefCell<Vec<Op>>,
        fail_key: Option<Hotkey>,
    }

    impl HotkeyPort for LogPort<'_> {
        fn register(&mut self, id: i32, hotkey: &Hotkey) -> bool {
            self.ops.borrow_mut().push(Op::Register(id));
            let taken = self
                .live
                .borrow()
                .iter()
                .any(|(other, key)| *other != id && key == hotkey);
            if taken || self.fail_key == Some(*hotkey) {
                return false;
            }
            self.live.borrow_mut().insert(id, *hotkey);
            true
        }

        fn unregister(&mut self, id: i32) {
            self.ops.borrow_mut().push(Op::Unregister(id));
            self.live.borrow_mut().remove(&id);
        }
    }

    #[test]
    fn new_hotkey_is_registered_before_save_and_old_combo_is_dropped_after() {
        let current = desired_bindings(&Config::default()).unwrap();
        let with_panel = desired_bindings(&Config {
            panel_hotkey: Some("Ctrl+Shift+P".into()),
            ..Config::default()
        })
        .unwrap();
        let changed = Config {
            search_hotkey: "Ctrl+Alt+N".into(),
            ..Config::default()
        };
        let next = desired_bindings(&changed).unwrap();
        let old_search = current[0].hotkey;
        let new_search = next[0].hotkey;
        let live = std::cell::RefCell::new(BTreeMap::new());
        let ops = std::cell::RefCell::new(Vec::new());
        let mut port = LogPort {
            live: &live,
            ops: &ops,
            fail_key: None,
        };
        apply_hotkeys(&mut port, &[], &with_panel).unwrap();
        ops.borrow_mut().clear();
        commit_hotkeys(&mut port, &with_panel, &next, || {
            assert_eq!(live.borrow().get(&SEARCH_HOTKEY_ID), Some(&new_search));
            assert!(live.borrow().contains_key(&PANEL_HOTKEY_ID));
            ops.borrow_mut().push(Op::Persist);
            Ok::<(), &str>(())
        })
        .unwrap();
        assert_eq!(
            ops.borrow().as_slice(),
            &[
                Op::Register(SEARCH_HOTKEY_ID),
                Op::Persist,
                Op::Unregister(PANEL_HOTKEY_ID),
            ]
        );
        assert_eq!(live.borrow().get(&SEARCH_HOTKEY_ID), Some(&new_search));
        assert_ne!(live.borrow().get(&SEARCH_HOTKEY_ID), Some(&old_search));
        assert!(!live.borrow().contains_key(&PANEL_HOTKEY_ID));
    }

    #[test]
    fn occupied_change_keeps_the_old_hotkey_and_does_not_persist() {
        let current = desired_bindings(&Config::default()).unwrap();
        let changed = Config {
            search_hotkey: "Ctrl+Alt+N".into(),
            ..Config::default()
        };
        let next = desired_bindings(&changed).unwrap();
        let live = std::cell::RefCell::new(BTreeMap::new());
        let ops = std::cell::RefCell::new(Vec::new());
        let mut port = LogPort {
            live: &live,
            ops: &ops,
            fail_key: None,
        };
        apply_hotkeys(&mut port, &[], &current).unwrap();
        port.fail_key = Some(next[0].hotkey);
        ops.borrow_mut().clear();
        let mut persisted = false;
        let err = commit_hotkeys(&mut port, &current, &next, || {
            persisted = true;
            Ok::<(), &str>(())
        })
        .unwrap_err();
        assert!(matches!(err, CommitError::Occupied(_)));
        assert!(!persisted);
        assert_eq!(
            live.borrow().get(&SEARCH_HOTKEY_ID),
            Some(&current[0].hotkey)
        );
        assert_eq!(ops.borrow().as_slice(), &[Op::Register(SEARCH_HOTKEY_ID)]);
    }

    #[test]
    fn failed_save_restores_the_previous_hotkeys() {
        let current = desired_bindings(&Config::default()).unwrap();
        let changed = Config {
            panel_hotkey: Some("Ctrl+Shift+P".into()),
            ..Config::default()
        };
        let next = desired_bindings(&changed).unwrap();
        let live = std::cell::RefCell::new(BTreeMap::new());
        let ops = std::cell::RefCell::new(Vec::new());
        let mut port = LogPort {
            live: &live,
            ops: &ops,
            fail_key: None,
        };
        apply_hotkeys(&mut port, &[], &current).unwrap();
        ops.borrow_mut().clear();
        let err = commit_hotkeys(&mut port, &current, &next, || Err("写盘失败")).unwrap_err();
        assert!(matches!(err, CommitError::Persist("写盘失败")));
        assert_eq!(*live.borrow(), live_map(&current));
        assert!(!live.borrow().contains_key(&PANEL_HOTKEY_ID));
        assert_eq!(
            ops.borrow().as_slice(),
            &[
                Op::Register(PANEL_HOTKEY_ID),
                Op::Unregister(PANEL_HOTKEY_ID),
            ]
        );
    }

    fn live_map(bindings: &[RegisteredHotkey]) -> BTreeMap<i32, Hotkey> {
        bindings.iter().map(|item| (item.id, item.hotkey)).collect()
    }

    #[test]
    fn swapping_our_hotkeys_is_not_reported_as_occupied() {
        let current = desired_bindings(&Config {
            panel_hotkey: Some("Ctrl+Alt+N".into()),
            ..Config::default()
        })
        .unwrap();
        let swapped = desired_bindings(&Config {
            search_hotkey: "Ctrl+Alt+N".into(),
            panel_hotkey: Some("Ctrl+Alt+M".into()),
            ..Config::default()
        })
        .unwrap();
        let live = std::cell::RefCell::new(BTreeMap::new());
        let ops = std::cell::RefCell::new(Vec::new());
        let mut port = LogPort {
            live: &live,
            ops: &ops,
            fail_key: None,
        };
        apply_hotkeys(&mut port, &[], &current).unwrap();
        ops.borrow_mut().clear();
        commit_hotkeys(&mut port, &current, &swapped, || {
            assert_eq!(*live.borrow(), live_map(&swapped));
            ops.borrow_mut().push(Op::Persist);
            Ok::<(), &str>(())
        })
        .unwrap();
        assert_eq!(
            ops.borrow().as_slice(),
            &[
                Op::Unregister(PANEL_HOTKEY_ID),
                Op::Register(SEARCH_HOTKEY_ID),
                Op::Register(PANEL_HOTKEY_ID),
                Op::Persist,
            ]
        );
        assert_eq!(*live.borrow(), live_map(&swapped));
    }

    #[test]
    fn moving_one_combo_onto_the_other_id_keeps_both_registered() {
        let current = desired_bindings(&Config {
            panel_hotkey: Some("Ctrl+Shift+P".into()),
            ..Config::default()
        })
        .unwrap();
        let next = desired_bindings(&Config {
            search_hotkey: "Ctrl+Alt+N".into(),
            panel_hotkey: Some("Ctrl+Alt+M".into()),
            ..Config::default()
        })
        .unwrap();
        let live = std::cell::RefCell::new(BTreeMap::new());
        let ops = std::cell::RefCell::new(Vec::new());
        let mut port = LogPort {
            live: &live,
            ops: &ops,
            fail_key: None,
        };
        apply_hotkeys(&mut port, &[], &current).unwrap();
        ops.borrow_mut().clear();
        commit_hotkeys(&mut port, &current, &next, || {
            ops.borrow_mut().push(Op::Persist);
            Ok::<(), &str>(())
        })
        .unwrap();
        assert_eq!(*live.borrow(), live_map(&next));
        let recorded = ops.borrow();
        let persist = recorded
            .iter()
            .position(|op| *op == Op::Persist)
            .expect("persist");
        assert!(recorded[..persist].contains(&Op::Register(PANEL_HOTKEY_ID)));
    }

    #[test]
    fn failed_swap_restores_both_hotkeys_and_does_not_persist() {
        let current = desired_bindings(&Config {
            panel_hotkey: Some("Ctrl+Alt+N".into()),
            ..Config::default()
        })
        .unwrap();
        let swapped = desired_bindings(&Config {
            search_hotkey: "Ctrl+Alt+N".into(),
            panel_hotkey: Some("Ctrl+Alt+K".into()),
            ..Config::default()
        })
        .unwrap();
        let live = std::cell::RefCell::new(BTreeMap::new());
        let ops = std::cell::RefCell::new(Vec::new());
        let mut port = LogPort {
            live: &live,
            ops: &ops,
            fail_key: None,
        };
        apply_hotkeys(&mut port, &[], &current).unwrap();
        ops.borrow_mut().clear();
        port.fail_key = Some(swapped[1].hotkey);
        let mut persisted = false;
        let err = commit_hotkeys(&mut port, &current, &swapped, || {
            persisted = true;
            Ok::<(), &str>(())
        })
        .unwrap_err();
        assert!(matches!(err, CommitError::Occupied(_)));
        assert!(!persisted);
        assert_eq!(*live.borrow(), live_map(&current));
        assert_eq!(
            ops.borrow().as_slice(),
            &[
                Op::Unregister(PANEL_HOTKEY_ID),
                Op::Register(SEARCH_HOTKEY_ID),
                Op::Register(PANEL_HOTKEY_ID),
                Op::Register(SEARCH_HOTKEY_ID),
                Op::Register(PANEL_HOTKEY_ID),
            ]
        );
    }

    #[test]
    fn save_hotkeys_writes_only_after_register_and_occupied_keeps_the_file() {
        let (_temp, store, commands) = open_config();
        commands.replace(commands.current()).unwrap();
        let current = desired_bindings(&commands.current()).unwrap();
        let path = store
            .document_path(&crate::storage::DocumentId::Config)
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        let mut port = MapPort {
            live: BTreeMap::new(),
            fail_key: None,
        };
        apply_hotkeys(&mut port, &[], &current).unwrap();

        let saved = save_hotkeys(&mut port, &commands, &current, "Ctrl+Alt+N", None).unwrap();
        assert_eq!(commands.current().search_hotkey, "Ctrl+Alt+N");
        assert_eq!(port.live.get(&SEARCH_HOTKEY_ID), Some(&saved[0].hotkey));
        assert_ne!(std::fs::read(&path).unwrap(), before);

        let occupied = saved[0].hotkey;
        let written = std::fs::read(&path).unwrap();
        port.fail_key = Some({
            let mut again = commands.current();
            again.search_hotkey = "Ctrl+Alt+K".into();
            desired_bindings(&again).unwrap()[0].hotkey
        });
        let err = save_hotkeys(&mut port, &commands, &saved, "Ctrl+Alt+K", None).unwrap_err();
        assert_eq!(err.to_string(), "热键已被占用");
        assert_eq!(std::fs::read(&path).unwrap(), written);
        assert_eq!(commands.current().search_hotkey, "Ctrl+Alt+N");
        assert_eq!(port.live.get(&SEARCH_HOTKEY_ID), Some(&occupied));

        let err = save_hotkeys(&mut port, &commands, &saved, "Ctrl+Nope", None).unwrap_err();
        assert_eq!(err.to_string(), "热键无效");
        assert_eq!(std::fs::read(&path).unwrap(), written);
        assert_eq!(port.live.get(&SEARCH_HOTKEY_ID), Some(&occupied));
    }

    #[test]
    fn save_hotkeys_swaps_without_reporting_occupied() {
        let (_temp, store, commands) = open_config();
        let mut start = commands.current();
        start.panel_hotkey = Some("Ctrl+Alt+N".into());
        commands.replace(start).unwrap();
        let current = desired_bindings(&commands.current()).unwrap();
        let path = store
            .document_path(&crate::storage::DocumentId::Config)
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        let events = store.subscribe();
        let mut port = MapPort {
            live: BTreeMap::new(),
            fail_key: None,
        };
        apply_hotkeys(&mut port, &[], &current).unwrap();

        let saved = save_hotkeys(
            &mut port,
            &commands,
            &current,
            "Ctrl+Alt+N",
            Some("Ctrl+Alt+M"),
        )
        .unwrap();
        assert_eq!(
            events.try_recv().unwrap().kind,
            crate::storage::EntityKind::Config
        );
        assert_eq!(commands.current().search_hotkey, "Ctrl+Alt+N");
        assert_eq!(
            commands.current().panel_hotkey.as_deref(),
            Some("Ctrl+Alt+M")
        );
        assert_eq!(port.live, live_map(&saved));
        assert_ne!(std::fs::read(&path).unwrap(), before);
    }

    fn open_config() -> (TempDir, crate::storage::Store, ConfigCommands) {
        let temp = TempDir::new();
        let store = crate::storage::Store::open(crate::storage::StorePaths {
            data_dir: temp.path.join("data"),
            cache_dir: temp.path.join("cache"),
            user_profile: temp.path.join("profile"),
            local_app_data: temp.path.join("local"),
        })
        .unwrap();
        let commands = ConfigCommands::open(store.clone()).unwrap();
        (temp, store, commands)
    }

    struct TempDir {
        path: std::path::PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static SEQ: AtomicU64 = AtomicU64::new(0);
            let seq = SEQ.fetch_add(1, Ordering::Relaxed);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("lanwork-hotkey-{nanos}-{seq}"));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
