//! 把一组热键注册到系统。
//!
//! 真正的 `RegisterHotKey` 在外壳。运行中修改只走先注册、成功后写盘、再卸旧。
//! 本进程里与新组合冲突的注册要先卸掉再注册；失败则全部恢复。注册失败不写配置。
//!
//! 每次注册都用一个新的 id，不在同一个 id 上换组合：`RegisterHotKey` 在同一窗口、同一 id
//! 上再注册一次时，旧组合不会被替换，两个组合会同时有效（微软文档写明，只有 Windows Server 2003
//! 才是替换）。所以「搜索条」「面板」是角色（[`HotkeyRole`]），id 只是某一次注册的句柄。

use crate::config::{Config, ConfigCommands, ConfigError, Hotkey, normalize, parse_hotkey};

/// 一条热键在程序里的用途。窗口收到 `WM_HOTKEY` 时由 id 查到角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyRole {
    Search,
    Panel,
}

impl HotkeyRole {
    /// 设置页和日志里的名称。
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Search => "搜索条热键",
            Self::Panel => "面板热键",
        }
    }
}

/// 想要注册的一条热键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyBinding {
    pub role: HotkeyRole,
    pub hotkey: Hotkey,
}

/// 已经注册到系统的一条热键。`id` 是这次注册的句柄，同一时刻不重复；换组合时会换新的 id。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisteredHotkey {
    pub id: i32,
    pub role: HotkeyRole,
    pub hotkey: Hotkey,
}

/// 窗口收到 `WM_HOTKEY` 时，用 id 在已注册的热键里查角色。已经卸掉的 id 查不到。
#[must_use]
pub fn role_of(current: &[RegisteredHotkey], id: i32) -> Option<HotkeyRole> {
    current
        .iter()
        .find(|item| item.id == id)
        .map(|item| item.role)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindError {
    /// 系统拒绝注册。调用返回时，端口上的热键应仍是原来的一组。
    Occupied { role: HotkeyRole },
}

impl std::fmt::Display for BindError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Occupied { .. } => write!(f, "热键已被占用"),
        }
    }
}

/// 操作系统上的热键注册。`register` 返回 false 表示被占用或系统拒绝。
///
/// 实现按 `RegisterHotKey` 的语义：同一个 id 再注册会叠加，不会替换，所以调用方不对
/// 仍然有效的 id 再调用 `register`。
pub trait HotkeyPort {
    fn register(&mut self, id: i32, hotkey: &Hotkey) -> bool;
    fn unregister(&mut self, id: i32);
}

/// 从一份已经 [`crate::config::normalize`] 过的配置得到要注册的热键。
///
/// 面板热键为空时不产生第二条。
pub fn desired_bindings(config: &Config) -> Result<Vec<HotkeyBinding>, ConfigError> {
    let search = parse_hotkey(&config.search_hotkey)?;
    let panel = match config.panel_hotkey.as_deref() {
        None => None,
        Some(text) => Some(parse_hotkey(text)?),
    };
    if panel == Some(search) {
        return Err(ConfigError::SameHotkey);
    }
    let mut bindings = vec![HotkeyBinding {
        role: HotkeyRole::Search,
        hotkey: search,
    }];
    if let Some(hotkey) = panel {
        bindings.push(HotkeyBinding {
            role: HotkeyRole::Panel,
            hotkey,
        });
    }
    Ok(bindings)
}

/// 让端口上的热键变成 `next`，返回注册之后的一组。先注册新的，成功后再卸掉不再使用的旧注册。
///
/// 角色的组合变了就用新的 id 注册，旧 id 在新的注册成功之后才卸掉。
/// 新组合若已被本进程的另一条热键占用，先卸掉那条再注册。内容相同时不调用端口。
/// 注册失败时恢复这次动过的热键。
pub fn apply_hotkeys<P: HotkeyPort>(
    port: &mut P,
    current: &[RegisteredHotkey],
    next: &[HotkeyBinding],
) -> Result<Vec<RegisteredHotkey>, BindError> {
    let prepared = prepare(port, current, next)?;
    retire(port, &prepared);
    Ok(prepared.next)
}

/// 录入热键期间卸掉 `current`，否则按下已注册的组合会被系统当成热键，窗口收不到这次按键。
///
/// 只动端口，不写配置。调用方此后把「当前已注册」记成空，录入结束时用 [`restore_hotkeys`] 注册回去。
pub fn suspend_hotkeys<P: HotkeyPort>(port: &mut P, current: &[RegisteredHotkey]) {
    for item in current {
        port.unregister(item.id);
    }
}

/// [`restore_hotkeys`] 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Restored {
    /// 注册回去的热键，用的是新 id。
    pub registered: Vec<RegisteredHotkey>,
    /// 注册不上的角色，多半是录入期间被别的程序占了。
    pub failed: Vec<HotkeyRole>,
}

/// 把 [`suspend_hotkeys`] 卸掉的热键按 `wanted` 注册回去。每一条单独注册：
/// 有一条失败，其余仍然保留，失败的角色记在 [`Restored::failed`]。
///
/// 调用时本进程不应还有这些热键在端口上。
pub fn restore_hotkeys<P: HotkeyPort>(port: &mut P, wanted: &[HotkeyBinding]) -> Restored {
    let mut registered: Vec<RegisteredHotkey> = Vec::new();
    let mut failed = Vec::new();
    for item in wanted {
        let used: Vec<i32> = registered.iter().map(|done| done.id).collect();
        let id = fresh_id(&used);
        if port.register(id, &item.hotkey) {
            registered.push(RegisteredHotkey {
                id,
                role: item.role,
                hotkey: item.hotkey,
            });
        } else {
            failed.push(item.role);
        }
    }
    Restored { registered, failed }
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
    save_bindings(port, commands, current, search, panel, false)
}

/// 设置页改搜索条热键：面板热键保持配置里的值，只换搜索条的。
///
/// 走 [`save_hotkeys`] 的顺序：字符串不合法、与面板热键相同、被别的程序占用或写盘失败时，
/// 端口和 `config.json` 都保持原样。面板热键当前没有注册上（比如被别的程序占着）时，
/// 这次不去碰它，所以它不会拖住搜索条热键的保存。
pub fn save_search_hotkey<P: HotkeyPort>(
    port: &mut P,
    commands: &ConfigCommands,
    current: &[RegisteredHotkey],
    search: &str,
) -> Result<Vec<RegisteredHotkey>, SaveHotkeyError> {
    let panel = commands.current().panel_hotkey;
    save_bindings(port, commands, current, search, panel.as_deref(), true)
}

fn save_bindings<P: HotkeyPort>(
    port: &mut P,
    commands: &ConfigCommands,
    current: &[RegisteredHotkey],
    search: &str,
    panel: Option<&str>,
    leave_unregistered_panel: bool,
) -> Result<Vec<RegisteredHotkey>, SaveHotkeyError> {
    let mut next = commands.current();
    next.search_hotkey = search.to_owned();
    next.panel_hotkey = panel.map(str::to_owned);
    let next = normalize(next).map_err(SaveHotkeyError::Config)?;
    let mut bindings = desired_bindings(&next).map_err(SaveHotkeyError::Config)?;
    if leave_unregistered_panel && role_registered(current, HotkeyRole::Panel).is_none() {
        bindings.retain(|binding| binding.role != HotkeyRole::Panel);
    }
    commit_hotkeys(port, current, &bindings, || {
        commands.replace(next).map(|_| ())
    })
    .map_err(|err| match err {
        CommitError::Occupied(err) => SaveHotkeyError::Bind(err),
        CommitError::Persist(err) => SaveHotkeyError::Config(err),
    })
}

fn role_registered(current: &[RegisteredHotkey], role: HotkeyRole) -> Option<&RegisteredHotkey> {
    current.iter().find(|item| item.role == role)
}

#[derive(Debug)]
enum CommitError<E> {
    Occupied(BindError),
    Persist(E),
}

fn commit_hotkeys<P, E>(
    port: &mut P,
    current: &[RegisteredHotkey],
    next: &[HotkeyBinding],
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

/// 最小的、不在 `used` 里的正整数。
fn fresh_id(used: &[i32]) -> i32 {
    (1..).find(|id| !used.contains(id)).unwrap_or(i32::MAX)
}

fn prepare<P: HotkeyPort>(
    port: &mut P,
    current: &[RegisteredHotkey],
    next: &[HotkeyBinding],
) -> Result<Prepared, BindError> {
    if same(current, next) {
        return Ok(Prepared {
            next: current.to_vec(),
            undo: Vec::new(),
            retired: Vec::new(),
        });
    }
    let mut undo = Vec::new();
    let mut live = current.to_vec();
    let mut used: Vec<i32> = current.iter().map(|old| old.id).collect();
    let mut registered = Vec::new();
    for item in next {
        if let Some(kept) = live
            .iter()
            .find(|old| old.role == item.role && old.hotkey == item.hotkey)
        {
            registered.push(kept.clone());
            continue;
        }
        let conflicts: Vec<RegisteredHotkey> = live
            .iter()
            .filter(|old| old.role != item.role && old.hotkey == item.hotkey)
            .cloned()
            .collect();
        for conflict in conflicts {
            port.unregister(conflict.id);
            undo.push(Undo::Restore(conflict.clone()));
            live.retain(|old| old.id != conflict.id);
        }
        let id = fresh_id(&used);
        if !port.register(id, &item.hotkey) {
            undo_hotkeys(port, &undo);
            return Err(BindError::Occupied { role: item.role });
        }
        used.push(id);
        undo.push(Undo::DropNew(id));
        let added = RegisteredHotkey {
            id,
            role: item.role,
            hotkey: item.hotkey,
        };
        live.push(added.clone());
        registered.push(added);
    }
    let retired = current
        .iter()
        .filter(|old| !registered.iter().any(|item| item.id == old.id))
        .map(|old| old.id)
        .collect();
    Ok(Prepared {
        next: registered,
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

fn same(current: &[RegisteredHotkey], next: &[HotkeyBinding]) -> bool {
    current.len() == next.len()
        && current
            .iter()
            .zip(next)
            .all(|(left, right)| left.role == right.role && left.hotkey == right.hotkey)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    /// 按 `RegisterHotKey` 的真实语义做的端口：
    /// - 同一个组合已经有人注册（含本进程自己）时失败；
    /// - 同一个 id 再注册会叠加，不会替换，叠加的次数记在 `stacked`；
    /// - `unregister(id)` 只删掉一条该 id 的注册。
    struct Win32Port {
        live: Vec<(i32, Hotkey)>,
        held_by_others: Vec<Hotkey>,
        stacked: Vec<i32>,
        ops: Vec<Op>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Op {
        Register(i32),
        Unregister(i32),
        Persist,
    }

    impl Win32Port {
        fn new() -> Self {
            Self {
                live: Vec::new(),
                held_by_others: Vec::new(),
                stacked: Vec::new(),
                ops: Vec::new(),
            }
        }

        /// 现在按下哪些组合会发出 `WM_HOTKEY`。
        fn firing(&self) -> Vec<String> {
            let mut combos: Vec<String> =
                self.live.iter().map(|(_, key)| key.canonical()).collect();
            combos.sort();
            combos
        }

        fn ids(&self) -> Vec<i32> {
            let mut ids: Vec<i32> = self.live.iter().map(|(id, _)| *id).collect();
            ids.sort_unstable();
            ids
        }
    }

    impl HotkeyPort for Win32Port {
        fn register(&mut self, id: i32, hotkey: &Hotkey) -> bool {
            self.ops.push(Op::Register(id));
            if self.held_by_others.contains(hotkey)
                || self.live.iter().any(|(_, key)| key == hotkey)
            {
                return false;
            }
            if self.live.iter().any(|(other, _)| *other == id) {
                self.stacked.push(id);
            }
            self.live.push((id, *hotkey));
            true
        }

        fn unregister(&mut self, id: i32) {
            self.ops.push(Op::Unregister(id));
            if let Some(index) = self.live.iter().position(|(other, _)| *other == id) {
                self.live.remove(index);
            }
        }
    }

    fn config(search: &str, panel: Option<&str>) -> Config {
        Config {
            search_hotkey: search.into(),
            panel_hotkey: panel.map(str::to_owned),
            ..Config::default()
        }
    }

    fn bindings(search: &str, panel: Option<&str>) -> Vec<HotkeyBinding> {
        desired_bindings(&config(search, panel)).unwrap()
    }

    fn registered(
        port: &mut Win32Port,
        search: &str,
        panel: Option<&str>,
    ) -> Vec<RegisteredHotkey> {
        let current = apply_hotkeys(port, &[], &bindings(search, panel)).unwrap();
        port.ops.clear();
        current
    }

    fn combos(list: &[&str]) -> Vec<String> {
        let mut combos: Vec<String> = list.iter().map(|text| (*text).to_owned()).collect();
        combos.sort();
        combos
    }

    fn held(text: &str) -> Hotkey {
        parse_hotkey(text).unwrap()
    }

    #[test]
    fn changing_the_combo_registers_a_new_id_and_the_old_combo_stops_firing() {
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", None);
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+M"]));

        let next = commit_hotkeys(&mut port, &current, &bindings("Ctrl+Alt+K", None), || {
            Ok::<(), &str>(())
        })
        .unwrap();

        assert_eq!(port.firing(), combos(&["Ctrl+Alt+K"]));
        assert!(port.stacked.is_empty());
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].role, HotkeyRole::Search);
        assert_ne!(next[0].id, current[0].id);
        assert_eq!(port.ids(), vec![next[0].id]);
        assert_eq!(role_of(&next, next[0].id), Some(HotkeyRole::Search));
        assert_eq!(role_of(&next, current[0].id), None);
    }

    #[test]
    fn the_new_combo_is_registered_before_the_write_and_the_old_one_dropped_after() {
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", Some("Ctrl+Shift+P"));
        let old_search = current[0].id;

        let next = commit_hotkeys(
            &mut port,
            &current,
            &bindings("Ctrl+Alt+K", Some("Ctrl+Shift+P")),
            || Ok::<(), &str>(()),
        )
        .unwrap();
        let new_search = next[0].id;
        assert_eq!(
            port.ops,
            vec![Op::Register(new_search), Op::Unregister(old_search)],
            "先注册新的，写盘之后才卸旧的"
        );
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+K", "Ctrl+Shift+P"]));
        assert_eq!(next[1], current[1], "没动的面板热键不重新注册");
        assert!(port.stacked.is_empty());
    }

    #[test]
    fn both_combos_are_live_while_the_write_runs() {
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", None);
        let mut seen = Vec::new();
        let prepared = prepare(&mut port, &current, &bindings("Ctrl+Alt+K", None)).unwrap();
        seen.extend(port.firing());
        port.ops.push(Op::Persist);
        retire(&mut port, &prepared);
        assert_eq!(seen, combos(&["Ctrl+Alt+K", "Ctrl+Alt+M"]));
        assert_eq!(
            port.ops,
            vec![
                Op::Register(prepared.next[0].id),
                Op::Persist,
                Op::Unregister(current[0].id)
            ]
        );
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+K"]));
    }

    #[test]
    fn changing_again_and_again_leaves_exactly_one_combo_and_one_id() {
        let mut port = Win32Port::new();
        let mut current = registered(&mut port, "Ctrl+Alt+M", Some("Ctrl+Shift+P"));
        for text in ["Ctrl+Alt+K", "Ctrl+Alt+J", "Ctrl+Alt+M", "F9", "Ctrl+Alt+K"] {
            current =
                apply_hotkeys(&mut port, &current, &bindings(text, Some("Ctrl+Shift+P"))).unwrap();
            assert_eq!(port.firing(), combos(&[text, "Ctrl+Shift+P"]), "{text}");
            assert!(port.stacked.is_empty(), "{text}");
            assert_eq!(port.ids().len(), 2, "{text}");
        }
        let ids: Vec<i32> = current.iter().map(|item| item.id).collect();
        assert_eq!(port.ids(), {
            let mut sorted = ids;
            sorted.sort_unstable();
            sorted
        });
    }

    #[test]
    fn an_occupied_new_combo_keeps_the_old_one_and_does_not_persist() {
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", None);
        port.held_by_others.push(held("Ctrl+Alt+K"));

        let mut persisted = false;
        let err = commit_hotkeys(&mut port, &current, &bindings("Ctrl+Alt+K", None), || {
            persisted = true;
            Ok::<(), &str>(())
        })
        .unwrap_err();
        assert!(matches!(
            err,
            CommitError::Occupied(BindError::Occupied {
                role: HotkeyRole::Search
            })
        ));
        assert!(!persisted);
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+M"]));
        assert_eq!(port.ids(), vec![current[0].id]);
    }

    #[test]
    fn a_failed_write_drops_the_new_combo_and_keeps_the_old_one() {
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", None);

        let err = commit_hotkeys(&mut port, &current, &bindings("Ctrl+Alt+K", None), || {
            Err("写盘失败")
        })
        .unwrap_err();
        assert!(matches!(err, CommitError::Persist("写盘失败")));
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+M"]));
        assert_eq!(port.ids(), vec![current[0].id]);
        assert!(port.stacked.is_empty());
    }

    #[test]
    fn clearing_the_panel_hotkey_unregisters_it() {
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", Some("Ctrl+Shift+P"));
        let next = apply_hotkeys(&mut port, &current, &bindings("Ctrl+Alt+M", None)).unwrap();
        assert_eq!(next, vec![current[0].clone()]);
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+M"]));
    }

    #[test]
    fn adding_a_panel_hotkey_keeps_the_search_registration_untouched() {
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", None);
        let next = apply_hotkeys(
            &mut port,
            &current,
            &bindings("Ctrl+Alt+M", Some("Ctrl+Shift+P")),
        )
        .unwrap();
        assert_eq!(next[0], current[0]);
        assert_eq!(next[1].role, HotkeyRole::Panel);
        assert_eq!(port.ops, vec![Op::Register(next[1].id)]);
    }

    #[test]
    fn an_unchanged_set_does_not_touch_the_port() {
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", Some("Ctrl+Shift+P"));
        let next = apply_hotkeys(
            &mut port,
            &current,
            &bindings("Ctrl+Alt+M", Some("Ctrl+Shift+P")),
        )
        .unwrap();
        assert_eq!(next, current);
        assert!(port.ops.is_empty());
    }

    #[test]
    fn swapping_our_hotkeys_is_not_reported_as_occupied() {
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", Some("Ctrl+Alt+N"));
        let swapped = bindings("Ctrl+Alt+N", Some("Ctrl+Alt+M"));

        let next = commit_hotkeys(&mut port, &current, &swapped, || Ok::<(), &str>(())).unwrap();
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+M", "Ctrl+Alt+N"]));
        assert!(port.stacked.is_empty());
        assert_eq!(next[0].hotkey, held("Ctrl+Alt+N"));
        assert_eq!(next[1].hotkey, held("Ctrl+Alt+M"));
        assert_eq!(port.ids().len(), 2);
        assert!(
            next.iter()
                .all(|item| current.iter().all(|old| old.id != item.id)),
            "换了组合的角色都用新 id"
        );
    }

    #[test]
    fn moving_one_combo_onto_the_other_role_keeps_both_registered() {
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", Some("Ctrl+Shift+P"));
        let next = apply_hotkeys(
            &mut port,
            &current,
            &bindings("Ctrl+Alt+N", Some("Ctrl+Alt+M")),
        )
        .unwrap();
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+M", "Ctrl+Alt+N"]));
        assert!(port.stacked.is_empty());
        assert_eq!(next.len(), 2);
    }

    #[test]
    fn a_failed_swap_restores_both_hotkeys_and_does_not_persist() {
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", Some("Ctrl+Alt+N"));
        port.held_by_others.push(held("Ctrl+Alt+K"));

        let mut persisted = false;
        let err = commit_hotkeys(
            &mut port,
            &current,
            &bindings("Ctrl+Alt+N", Some("Ctrl+Alt+K")),
            || {
                persisted = true;
                Ok::<(), &str>(())
            },
        )
        .unwrap_err();
        assert!(matches!(err, CommitError::Occupied(_)));
        assert!(!persisted);
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+M", "Ctrl+Alt+N"]));
        let mut ids = port.ids();
        let mut expected: Vec<i32> = current.iter().map(|item| item.id).collect();
        ids.sort_unstable();
        expected.sort_unstable();
        assert_eq!(ids, expected);
        assert!(port.stacked.is_empty());
    }

    #[test]
    fn a_failed_write_after_a_swap_restores_both_hotkeys() {
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", Some("Ctrl+Alt+N"));
        let err = commit_hotkeys(
            &mut port,
            &current,
            &bindings("Ctrl+Alt+N", Some("Ctrl+Alt+M")),
            || Err("写盘失败"),
        )
        .unwrap_err();
        assert!(matches!(err, CommitError::Persist(_)));
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+M", "Ctrl+Alt+N"]));
        assert!(port.stacked.is_empty());
        let by_role = |role| {
            port.live
                .iter()
                .find(|(id, _)| role_of(&current, *id) == Some(role))
                .map(|(_, key)| key.canonical())
        };
        assert_eq!(by_role(HotkeyRole::Search).as_deref(), Some("Ctrl+Alt+M"));
        assert_eq!(by_role(HotkeyRole::Panel).as_deref(), Some("Ctrl+Alt+N"));
    }

    #[test]
    fn the_port_model_really_stacks_a_reused_id() {
        let mut port = Win32Port::new();
        assert!(port.register(1, &held("Ctrl+Alt+M")));
        assert!(port.register(1, &held("Ctrl+Alt+K")));
        assert_eq!(port.stacked, vec![1]);
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+K", "Ctrl+Alt+M"]));
        port.unregister(1);
        assert_eq!(port.firing().len(), 1, "只卸掉一条，另一条仍然有效");
        assert!(
            !port.register(2, &held("Ctrl+Alt+K")),
            "同一个组合不能注册两次"
        );
    }

    #[test]
    fn suspend_frees_every_key_and_restore_registers_them_again() {
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", Some("Ctrl+Shift+P"));
        suspend_hotkeys(&mut port, &current);
        assert!(port.live.is_empty());
        suspend_hotkeys(&mut port, &current);
        assert!(port.live.is_empty());

        let restored = restore_hotkeys(&mut port, &bindings("Ctrl+Alt+M", Some("Ctrl+Shift+P")));
        assert!(restored.failed.is_empty());
        assert_eq!(restored.registered.len(), 2);
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+M", "Ctrl+Shift+P"]));
        assert!(port.stacked.is_empty());
        assert_eq!(restored.registered[0].role, HotkeyRole::Search);
        assert_eq!(restored.registered[1].role, HotkeyRole::Panel);
    }

    #[test]
    fn restore_keeps_what_registered_when_one_was_taken_meanwhile() {
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", Some("Ctrl+Shift+P"));
        suspend_hotkeys(&mut port, &current);
        port.held_by_others.push(held("Ctrl+Shift+P"));
        let restored = restore_hotkeys(&mut port, &bindings("Ctrl+Alt+M", Some("Ctrl+Shift+P")));
        assert_eq!(restored.failed, vec![HotkeyRole::Panel]);
        assert_eq!(restored.registered.len(), 1);
        assert_eq!(restored.registered[0].role, HotkeyRole::Search);
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+M"]));

        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", Some("Ctrl+Shift+P"));
        suspend_hotkeys(&mut port, &current);
        port.held_by_others.push(held("Ctrl+Alt+M"));
        let restored = restore_hotkeys(&mut port, &bindings("Ctrl+Alt+M", Some("Ctrl+Shift+P")));
        assert_eq!(restored.failed, vec![HotkeyRole::Search]);
        assert_eq!(
            port.firing(),
            combos(&["Ctrl+Shift+P"]),
            "面板那条仍然注册上"
        );
    }

    #[test]
    fn restore_of_nothing_registers_nothing() {
        let mut port = Win32Port::new();
        let restored = restore_hotkeys(&mut port, &[]);
        assert!(restored.registered.is_empty() && restored.failed.is_empty());
        assert!(port.ops.is_empty());
    }

    #[test]
    fn save_hotkeys_writes_only_after_register_and_occupied_keeps_the_file() {
        let (_temp, store, commands) = open_config();
        commands.replace(commands.current()).unwrap();
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", None);
        let path = store
            .document_path(&crate::storage::DocumentId::Config)
            .unwrap();
        let before = std::fs::read(&path).unwrap();

        let saved = save_hotkeys(&mut port, &commands, &current, "Ctrl+Alt+N", None).unwrap();
        assert_eq!(commands.current().search_hotkey, "Ctrl+Alt+N");
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+N"]));
        assert_ne!(saved[0].id, current[0].id);
        assert_ne!(std::fs::read(&path).unwrap(), before);
        assert!(port.stacked.is_empty());

        let written = std::fs::read(&path).unwrap();
        port.held_by_others.push(held("Ctrl+Alt+K"));
        let err = save_hotkeys(&mut port, &commands, &saved, "Ctrl+Alt+K", None).unwrap_err();
        assert_eq!(err.to_string(), "热键已被占用");
        assert_eq!(std::fs::read(&path).unwrap(), written);
        assert_eq!(commands.current().search_hotkey, "Ctrl+Alt+N");
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+N"]));

        let err = save_hotkeys(&mut port, &commands, &saved, "Ctrl+Nope", None).unwrap_err();
        assert_eq!(err.to_string(), "热键无效");
        assert_eq!(std::fs::read(&path).unwrap(), written);
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+N"]));
    }

    #[test]
    fn save_hotkeys_swaps_without_reporting_occupied() {
        let (_temp, store, commands) = open_config();
        let mut start = commands.current();
        start.panel_hotkey = Some("Ctrl+Alt+N".into());
        commands.replace(start).unwrap();
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", Some("Ctrl+Alt+N"));
        let path = store
            .document_path(&crate::storage::DocumentId::Config)
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        let events = store.subscribe();

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
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+M", "Ctrl+Alt+N"]));
        assert_eq!(saved.len(), 2);
        assert!(port.stacked.is_empty());
        assert_ne!(std::fs::read(&path).unwrap(), before);
    }

    fn config_with_panel_hotkey() -> (TempDir, crate::storage::Store, ConfigCommands) {
        let (temp, store, commands) = open_config();
        let mut start = commands.current();
        start.panel_hotkey = Some("Ctrl+Shift+P".into());
        commands.replace(start).unwrap();
        (temp, store, commands)
    }

    #[test]
    fn recording_pressing_the_same_combo_again_changes_nothing() {
        let (_temp, store, commands) = open_config();
        commands.replace(commands.current()).unwrap();
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", None);
        let path = store
            .document_path(&crate::storage::DocumentId::Config)
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        let events = store.subscribe();

        suspend_hotkeys(&mut port, &current);
        let restored = restore_hotkeys(&mut port, &desired_bindings(&commands.current()).unwrap());
        let saved =
            save_search_hotkey(&mut port, &commands, &restored.registered, "Ctrl+Alt+M").unwrap();

        assert_eq!(saved, restored.registered);
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+M"]));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn search_hotkey_save_keeps_the_panel_hotkey_and_refuses_the_same_combo() {
        let (_temp, store, commands) = config_with_panel_hotkey();
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", Some("Ctrl+Shift+P"));
        let path = store
            .document_path(&crate::storage::DocumentId::Config)
            .unwrap();
        let before = std::fs::read(&path).unwrap();

        let err = save_search_hotkey(&mut port, &commands, &current, "shift+ctrl+p").unwrap_err();
        assert_eq!(err.to_string(), "搜索条热键与面板热键相同");
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+M", "Ctrl+Shift+P"]));
        assert_eq!(commands.current().search_hotkey, "Ctrl+Alt+M");

        let saved = save_search_hotkey(&mut port, &commands, &current, "Ctrl+Alt+K").unwrap();
        assert_eq!(commands.current().search_hotkey, "Ctrl+Alt+K");
        assert_eq!(
            commands.current().panel_hotkey.as_deref(),
            Some("Ctrl+Shift+P")
        );
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+K", "Ctrl+Shift+P"]));
        assert_eq!(saved[1], current[1]);
        assert!(port.stacked.is_empty());
    }

    #[test]
    fn a_panel_hotkey_that_never_registered_does_not_block_saving_the_search_hotkey() {
        let (_temp, _store, commands) = config_with_panel_hotkey();
        let mut port = Win32Port::new();
        port.held_by_others.push(held("Ctrl+Shift+P"));
        let current = restore_hotkeys(&mut port, &desired_bindings(&commands.current()).unwrap());
        assert_eq!(current.failed, vec![HotkeyRole::Panel]);

        let saved =
            save_search_hotkey(&mut port, &commands, &current.registered, "Ctrl+Alt+K").unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].role, HotkeyRole::Search);
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+K"]));
        assert_eq!(commands.current().search_hotkey, "Ctrl+Alt+K");
        assert_eq!(
            commands.current().panel_hotkey.as_deref(),
            Some("Ctrl+Shift+P"),
            "配置里的面板热键不被改动"
        );
    }

    #[test]
    fn an_invalid_hotkey_leaves_the_working_one_registered_and_the_file_alone() {
        let (_temp, store, commands) = open_config();
        commands.replace(commands.current()).unwrap();
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", None);
        let path = store
            .document_path(&crate::storage::DocumentId::Config)
            .unwrap();
        let before = std::fs::read(&path).unwrap();

        for text in [
            "",
            "   ",
            "Ctrl+Nope",
            "Space",
            "M",
            "5",
            "Ctrl+Alt",
            "Ctrl+Alt+M+K",
        ] {
            let err = save_search_hotkey(&mut port, &commands, &current, text).unwrap_err();
            assert!(
                matches!(err.to_string().as_str(), "热键无效" | "搜索条热键不能为空"),
                "{text}: {err}"
            );
            assert_eq!(port.firing(), combos(&["Ctrl+Alt+M"]), "{text}");
            assert_eq!(std::fs::read(&path).unwrap(), before, "{text}");
            assert_eq!(commands.current().search_hotkey, "Ctrl+Alt+M", "{text}");
        }
    }

    #[test]
    fn widened_hotkeys_save_and_bare_space_does_not() {
        let (_temp, _store, commands) = open_config();
        commands.replace(commands.current()).unwrap();
        let mut port = Win32Port::new();
        let mut current = registered(&mut port, "Ctrl+Alt+M", None);
        for text in ["Ctrl+Space", "Alt+Left", "Ctrl+Shift+/", "Ctrl+Numpad1"] {
            current = save_search_hotkey(&mut port, &commands, &current, text).unwrap();
            let canonical = parse_hotkey(text).unwrap().canonical();
            assert_eq!(commands.current().search_hotkey, canonical, "{text}");
            assert_eq!(port.firing(), combos(&[canonical.as_str()]), "{text}");
        }
        let err = save_search_hotkey(&mut port, &commands, &current, "Space").unwrap_err();
        assert_eq!(err.to_string(), "热键无效");
        assert_eq!(commands.current().search_hotkey, "Ctrl+Numpad1");
        assert_eq!(port.firing(), combos(&["Ctrl+Numpad1"]));
    }

    #[test]
    fn a_failed_config_write_unregisters_the_new_key_and_keeps_the_old_one() {
        let (_temp, store, commands) = open_config();
        commands.replace(commands.current()).unwrap();
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", None);
        let path = store
            .document_path(&crate::storage::DocumentId::Config)
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        std::fs::create_dir(path.with_file_name("config.json.tmp")).unwrap();
        let events = store.subscribe();

        let err = save_search_hotkey(&mut port, &commands, &current, "Ctrl+Alt+K").unwrap_err();
        assert!(
            matches!(err, SaveHotkeyError::Config(ConfigError::Store(_))),
            "{err}"
        );
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+M"]));
        assert_eq!(port.ids(), vec![current[0].id]);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(commands.current().search_hotkey, "Ctrl+Alt+M");
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn saving_a_key_another_program_holds_keeps_the_old_registration_and_file() {
        let (_temp, store, commands) = open_config();
        commands.replace(commands.current()).unwrap();
        let mut port = Win32Port::new();
        let current = registered(&mut port, "Ctrl+Alt+M", None);
        let path = store
            .document_path(&crate::storage::DocumentId::Config)
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        port.held_by_others.push(held("Ctrl+Alt+K"));

        let err = save_search_hotkey(&mut port, &commands, &current, "Ctrl+Alt+K").unwrap_err();
        assert!(matches!(err, SaveHotkeyError::Bind(_)));
        assert_eq!(err.to_string(), "热键已被占用");
        assert_eq!(port.firing(), combos(&["Ctrl+Alt+M"]));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_eq!(commands.current().search_hotkey, "Ctrl+Alt+M");
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
