//! 应用菜单的清理、官方卸载和残留清单。
//!
//! 搜索条和应用墙各持有一份对话框。卸载程序在前台时，打开它的那一扇窗口留着。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lanwork_core::apps::{
    AppEntry, AppSource, Leftover, LeftoverKind, PreparedUninstall, RemovalPlan, UninstallEnd,
    after_uninstall, launch_key, launch_uninstall, machine_roots, observe_leftovers,
    plan_leftovers, read_uninstall_records, removal_plan,
};
use lanwork_core::dispatch::Dispatch;

static HOLD: AtomicBool = AtomicBool::new(false);

pub(crate) fn held() -> bool {
    HOLD.load(Ordering::Acquire)
}

pub(crate) fn set_hold(hold: bool) {
    HOLD.store(hold, Ordering::Release);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RemovalPhase {
    Confirm,
    Checklist,
}

#[derive(Debug, Clone)]
pub(crate) struct RemovalRow {
    pub kind: LeftoverKind,
    pub location: String,
    pub checked: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct RemovalDialog {
    pub phase: RemovalPhase,
    pub heading: String,
    pub busy: bool,
    pub entry: AppEntry,
    pub prepared: PreparedUninstall,
    pub rows: Vec<RemovalRow>,
}

impl RemovalDialog {
    pub(crate) fn confirm(entry: AppEntry, prepared: PreparedUninstall) -> Self {
        Self {
            phase: RemovalPhase::Confirm,
            heading: entry.name.clone(),
            busy: false,
            entry,
            prepared,
            rows: Vec::new(),
        }
    }

    pub(crate) fn confirm_enabled(&self) -> bool {
        if self.busy {
            return false;
        }
        match self.phase {
            RemovalPhase::Confirm => true,
            RemovalPhase::Checklist => self.rows.iter().any(|row| row.checked),
        }
    }

    pub(crate) fn confirm_label(&self) -> &'static str {
        match self.phase {
            RemovalPhase::Confirm => "卸载",
            RemovalPhase::Checklist => "删除",
        }
    }

    pub(crate) fn body(&self) -> &'static str {
        match self.phase {
            RemovalPhase::Confirm => "会启动这个应用自己的卸载程序。",
            RemovalPhase::Checklist => "",
        }
    }
}

pub(crate) fn toggle(dialog: &mut RemovalDialog, index: usize) {
    if dialog.busy || dialog.phase != RemovalPhase::Checklist {
        return;
    }
    if let Some(row) = dialog.rows.get_mut(index) {
        row.checked = !row.checked;
        row.error = None;
    }
}

/// 打开菜单时用的计划。登记读失败时卸载项不可用，并带回原因。
pub(crate) fn plan_for(entry: &AppEntry) -> (RemovalPlan, Option<String>) {
    let (records, warning) = match read_uninstall_records() {
        Ok(records) => (records, None),
        Err(message) => (Vec::new(), Some(message)),
    };
    (removal_plan(entry, &records, &machine_roots()), warning)
}

#[derive(Debug)]
pub(crate) struct UninstallDone {
    pub error: Option<String>,
    pub rows: Vec<RemovalRow>,
    pub refreshed: bool,
}

pub(crate) fn run_uninstall(
    dispatch: &Dispatch,
    entry: &AppEntry,
    prepared: &PreparedUninstall,
) -> UninstallDone {
    let end = launch_uninstall(&prepared.command);
    if let UninstallEnd::Failed(message) = end {
        return UninstallDone {
            error: Some(message),
            rows: Vec::new(),
            refreshed: false,
        };
    }
    if let Err(err) = dispatch.refresh_apps() {
        return UninstallDone {
            error: Some(format!("刷新应用索引失败：{err}")),
            rows: Vec::new(),
            refreshed: false,
        };
    }
    let gone = indexed_gone(dispatch, entry);
    let decision = after_uninstall(&end, gone);
    if !decision.scan {
        return UninstallDone {
            error: decision.error,
            rows: Vec::new(),
            refreshed: decision.refresh,
        };
    }
    let roots = machine_roots();
    let mut footprint = prepared.footprint.clone();
    footprint.target_exe_present = footprint
        .target_exe
        .as_ref()
        .is_some_and(|path| path.is_file());
    let observed = observe_leftovers(&footprint, &roots);
    let rows = plan_leftovers(&footprint, &observed, &roots)
        .into_iter()
        .map(row_from_leftover)
        .collect();
    UninstallDone {
        error: decision.error,
        rows,
        refreshed: true,
    }
}

pub(crate) fn run_delete(rows: Vec<RemovalRow>) -> Vec<RemovalRow> {
    let roots = machine_roots();
    let mut left = Vec::new();
    for row in rows {
        if !row.checked {
            left.push(row);
            continue;
        }
        match lanwork_core::apps::delete_leftover(row.kind, &row.location, &roots) {
            Ok(()) => {}
            Err(message) => left.push(RemovalRow {
                error: Some(message),
                checked: true,
                ..row
            }),
        }
    }
    left
}

pub(crate) fn run_clean(dispatch: &Dispatch, path: &Path) -> Result<(), String> {
    let roots = machine_roots();
    let location = path.to_string_lossy();
    if !lanwork_core::apps::deletion_allowed(LeftoverKind::Shortcut, &location, &roots) {
        return Err("这个开始菜单入口不能删除".to_owned());
    }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("删不掉这个开始菜单入口：{err}")),
    }
    dispatch
        .refresh_start_menu()
        .map_err(|err| format!("刷新开始菜单失败：{err}"))
}

pub(crate) fn spawn_uninstall(
    dispatch: Arc<Dispatch>,
    entry: AppEntry,
    prepared: PreparedUninstall,
    on_bar: bool,
) -> bool {
    let spawned = std::thread::Builder::new()
        .name("lanwork-uninstall".into())
        .spawn(move || {
            let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_uninstall(&dispatch, &entry, &prepared)
            }));
            let done = match done {
                Ok(done) => done,
                Err(_) => UninstallDone {
                    error: Some("卸载过程中断".to_owned()),
                    rows: Vec::new(),
                    refreshed: false,
                },
            };
            let _ = slint::invoke_from_event_loop(move || {
                if on_bar {
                    crate::searchbar::finish_uninstall(done);
                } else {
                    crate::appwall::finish_uninstall(done);
                }
            });
        });
    spawned.is_ok()
}

pub(crate) fn spawn_delete(rows: Vec<RemovalRow>, on_bar: bool) -> bool {
    let spawned = std::thread::Builder::new()
        .name("lanwork-leftover".into())
        .spawn(move || {
            let left = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_delete(rows)));
            let left = left.unwrap_or_else(|_| {
                vec![RemovalRow {
                    kind: LeftoverKind::Directory,
                    location: String::new(),
                    checked: false,
                    error: Some("删除过程中断".to_owned()),
                }]
            });
            let _ = slint::invoke_from_event_loop(move || {
                if on_bar {
                    crate::searchbar::finish_delete(left);
                } else {
                    crate::appwall::finish_delete(left);
                }
            });
        });
    spawned.is_ok()
}

pub(crate) fn spawn_clean(dispatch: Arc<Dispatch>, path: PathBuf, on_bar: bool) -> bool {
    let spawned = std::thread::Builder::new()
        .name("lanwork-clean".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_clean(&dispatch, &path)
            }));
            let result = match result {
                Ok(result) => result,
                Err(_) => Err("清理过程中断".to_owned()),
            };
            let _ = slint::invoke_from_event_loop(move || {
                if on_bar {
                    crate::searchbar::finish_clean(result);
                } else {
                    crate::appwall::finish_clean(result);
                }
            });
        });
    spawned.is_ok()
}

fn indexed_gone(dispatch: &Dispatch, entry: &AppEntry) -> bool {
    let key = launch_key(&entry.target);
    if entry.source == AppSource::StartMenu {
        !dispatch.start_menu_has(&key)
    } else {
        !dispatch.contains_launch_key(&key)
    }
}

fn row_from_leftover(item: Leftover) -> RemovalRow {
    RemovalRow {
        kind: item.kind,
        location: item.location,
        checked: item.checked,
        error: None,
    }
}
