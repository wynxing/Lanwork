//! 面板的设置页。左边四个分类，右边是当前分类。
//! 常规：开机启动、热角、搜索条热键、主题、网页搜索引擎。
//! 待办：安静时段、「稍后」天数。数据：当前数据目录、更换并迁移、立即备份、导出和导入。
//! 其他：关于与更新、隐藏的应用。
//!
//! 决定在 `lanwork_core::panel` 和 `lanwork_core::shell`：先写 `config.json`，成功后才写注册表、
//! 换热角；热键先注册新的、再写盘、再卸旧的。这里只把结果推给界面，并在失败时用配置里的原值重推，
//! 让控件回到原状。错误显示在面板底部的错误条，与其他标签一致。
//! 录入热键时已注册的热键由平台线程卸掉，按键由 `keycap` 在窗口上拦截。
//! 「录入中」只有一个出口 [`SettingsPage::end_recording`]：录到组合、Esc、再点一次、失焦、收起、
//! 切标签、关闭和进程退出都走它，它把卸掉的热键注册回去。

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use lanwork_core::backup::{self, BackupCommands, MigrateError, overview_text};
use lanwork_core::config::{
    CLOCK_FORMAT_ERROR, ConfigCommands, QuietHours, SNOOZE_DAYS_ERROR, default_quiet_hours,
    parse_clock,
};
use lanwork_core::dispatch::Dispatch;
use lanwork_core::panel::{
    change_hot_corner, change_launch_at_startup, change_quiet_hours, change_snooze_days,
    change_theme, change_web_search_engine, hot_corner_at, hot_corner_titles, parse_snooze_days,
    settings_rows, startup_error_text, theme_at, theme_titles, web_engine_at, web_engine_titles,
};
use lanwork_core::shell::{EndReason, HotkeyRecording, HotkeyRole, Recorded};
use lanwork_core::storage::{ENV_DATA_DIR, Store};
use lanwork_core::todos::{DEFAULT_DEFER_DAYS, MAX_DEFER_DAYS, MIN_DEFER_DAYS};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::keycap;
use crate::panel::with_settings;
use crate::platform::HotkeyControl;
use crate::registry::apply_startup;
use crate::{Panel, SettingsBackup, SettingsHiddenApp, SettingsPageData};

const CAPTURE_UNAVAILABLE: &str = "热键录入不可用";

/// 设置页要用的服务：配置，和平台线程上的热键注册。
pub(crate) struct SettingsServices {
    pub(crate) config: ConfigCommands,
    pub(crate) hotkeys: HotkeyControl,
    /// 启动时按配置没注册上的热键。
    pub(crate) unregistered: Vec<HotkeyRole>,
    /// 应用索引没打开时没有隐藏名单。
    pub(crate) apps: Option<Arc<Dispatch>>,
}

pub(crate) struct SettingsPage {
    commands: ConfigCommands,
    hotkeys: HotkeyControl,
    store: Store,
    /// 录入状态和没注册上的热键提示，决定在 core 里。收起面板不清提示，注册上之后才清。
    recording: HotkeyRecording,
    capture_ready: bool,
    apps: Option<Arc<Dispatch>>,
    hidden_keys: RefCell<Vec<String>>,
    field_reset: Cell<i32>,
    /// 迁移、备份、导出或导入在后台线程里跑。完成之前对应按钮不可再点。
    busy: Cell<bool>,
    /// 已经校验过、还等用户确认的导入包。
    pending_import: RefCell<Option<PathBuf>>,
    import_overview: RefCell<String>,
}

enum Hold {
    Migrate(&'static str),
    Backup,
    Export,
    Read,
    Import,
}

impl SettingsPage {
    pub(crate) fn new(ui: &Panel, services: SettingsServices, store: Store) -> Self {
        let titles: Vec<SharedString> = hot_corner_titles().into_iter().map(Into::into).collect();
        let themes: Vec<SharedString> = theme_titles().into_iter().map(Into::into).collect();
        let engines: Vec<SharedString> = web_engine_titles().into_iter().map(Into::into).collect();
        let data = ui.global::<SettingsPageData>();
        data.set_corner_titles(ModelRc::from(std::rc::Rc::new(VecModel::from(titles))));
        data.set_theme_titles(ModelRc::from(std::rc::Rc::new(VecModel::from(themes))));
        data.set_engine_titles(ModelRc::from(std::rc::Rc::new(VecModel::from(engines))));
        data.on_set_launch_at_startup(|enabled| {
            with_settings(move |page, ui| page.set_launch_at_startup(ui, enabled));
        });
        data.on_choose_corner(|index| with_settings(move |page, ui| page.choose_corner(ui, index)));
        data.on_toggle_recording(|| with_settings(SettingsPage::toggle_recording));
        data.on_choose_theme(|index| with_settings(move |page, ui| page.choose_theme(ui, index)));
        data.on_choose_engine(|index| with_settings(move |page, ui| page.choose_engine(ui, index)));
        data.on_set_quiet(|enabled| with_settings(move |page, ui| page.set_quiet(ui, enabled)));
        data.on_commit_quiet_start(|text| {
            with_settings(move |page, ui| page.commit_quiet_start(ui, &text));
        });
        data.on_commit_quiet_end(|text| {
            with_settings(move |page, ui| page.commit_quiet_end(ui, &text));
        });
        data.on_commit_snooze(|text| {
            with_settings(move |page, ui| page.commit_snooze(ui, &text));
        });
        data.on_choose_section(|index| {
            with_settings(move |page, ui| page.choose_section(ui, index));
        });
        data.on_choose_data_dir(|| with_settings(SettingsPage::choose_data_dir));
        data.on_backup_now(|| with_settings(SettingsPage::backup_now));
        data.on_export_clicked(|| with_settings(SettingsPage::export_clicked));
        data.on_import_clicked(|| with_settings(SettingsPage::import_clicked));
        data.on_confirm_import(|| with_settings(SettingsPage::confirm_import));
        data.on_cancel_import(|| with_settings(SettingsPage::cancel_import));
        data.on_update_clicked(crate::update::on_clicked);
        crate::update::push_into(ui);
        data.on_restore_hidden(|index| {
            with_settings(move |page, ui| page.restore_hidden(ui, index));
        });
        let SettingsServices {
            config,
            hotkeys,
            unregistered,
            apps,
        } = services;
        let page = Self {
            commands: config,
            hotkeys,
            store,
            recording: HotkeyRecording::new(unregistered),
            capture_ready: false,
            apps,
            hidden_keys: RefCell::new(Vec::new()),
            field_reset: Cell::new(0),
            busy: Cell::new(false),
            pending_import: RefCell::new(None),
            import_overview: RefCell::new(String::new()),
        };
        page.push(ui);
        page
    }

    /// 面板窗口创建好之后，按键子类装没装上。没装上时不能录入，否则会卸了热键却收不到按键。
    pub(crate) fn set_capture_ready(&mut self, ready: bool) {
        self.capture_ready = ready;
    }

    /// 切到设置标签，或面板在设置标签上显示时调用。
    pub(crate) fn entered(&mut self, ui: &Panel) {
        self.cancel_recording(ui);
        self.push(ui);
    }

    /// 离开设置标签、面板收起或失焦、进程退出时调用。正在录入就放弃，并把热键注册回去。
    pub(crate) fn cancel_recording(&mut self, ui: &Panel) {
        self.end_recording(ui, EndReason::Abandon);
    }

    /// 面板错误条要显示的热键提示：配置里有、此刻没注册上的热键。没有时为空。
    pub(crate) fn hotkey_error(&self) -> Option<String> {
        self.recording.hotkey_error()
    }

    /// 处理模式「稍后」的天数。读内存里的 `processSnoozeDays`，与设置页「稍后」写入的是同一字段。
    /// 文件没写这个字段时是 3。超出 1 至 30 时也用 3。
    pub(crate) fn process_snooze_days(&self) -> u32 {
        let days = self.commands.current().process_snooze_days;
        if (MIN_DEFER_DAYS..=MAX_DEFER_DAYS).contains(&days) {
            days
        } else {
            DEFAULT_DEFER_DAYS
        }
    }

    /// 「录入中」状态的唯一出口，见 [`HotkeyRecording::end`]。没在录入时什么也不做。
    ///
    /// 先让平台线程把卸掉的热键按配置注册回去（逐条，一条失败不影响其余），再保存录到的组合。
    /// 注册不上的写日志，记下来直到下次注册上。
    fn end_recording(&mut self, ui: &Panel, reason: EndReason) {
        if reason == EndReason::Abandon {
            keycap::reset();
        }
        let Some(ending) = self.recording.end(reason) else {
            return;
        };
        let report = self
            .hotkeys
            .end_recording(self.commands.clone(), ending.save.clone());
        let result = self.recording.finish(&ending, report);
        for role in self.recording.unregistered() {
            self.store
                .log_warn(&format!("设置页：{}没有注册上：热键已被占用", role.title()));
        }
        self.finish(ui, result);
    }

    fn push(&self, ui: &Panel) {
        let rows = settings_rows(&self.commands.current());
        let data = ui.global::<SettingsPageData>();
        data.set_launch_at_startup(rows.launch_at_startup);
        data.set_corner(i32::try_from(rows.hot_corner).unwrap_or(0));
        data.set_hotkey(rows.search_hotkey.into());
        data.set_recording(self.recording.is_active());
        data.set_theme(i32::try_from(rows.theme).unwrap_or(2));
        data.set_engine(i32::try_from(rows.web_search_engine).unwrap_or(0));
        data.set_quiet_on(rows.quiet_on);
        data.set_quiet_start(rows.quiet_start.into());
        data.set_quiet_end(rows.quiet_end.into());
        data.set_snooze_days(rows.snooze_days.into());
        data.set_data_dir(self.store.data_dir().display().to_string().into());
        data.set_import_recovered(crate::host::import_was_recovered());
        if !self.busy.get() {
            self.arm_idle(ui);
            self.push_backups(ui);
        }
        let reset = self.field_reset.get().wrapping_add(1);
        self.field_reset.set(reset);
        data.set_field_reset(reset);
        self.push_hidden(ui);
    }

    /// 选择文件夹。取消就什么也不做。选中之后才开始迁移，路径要等切换成功才换。
    fn choose_data_dir(&mut self, ui: &Panel) {
        if self.busy.get() {
            return;
        }
        self.cancel_recording(ui);
        let Some(hwnd) = crate::bar_win::hwnd_of(ui.window()) else {
            self.finish(ui, Err("不能选择文件夹".to_owned()));
            return;
        };
        match crate::folder_pick::pick_folder(hwnd) {
            Ok(None) => {}
            Ok(Some(path)) => self.begin_migrate(ui, path),
            Err(message) => self.finish(ui, Err(message)),
        }
    }

    fn begin_migrate(&mut self, ui: &Panel, target: std::path::PathBuf) {
        self.hold(ui, Hold::Migrate("正在复制"));
        let store = self.store.clone();
        let env_blocks = std::env::var(ENV_DATA_DIR)
            .ok()
            .is_some_and(|value| !value.trim().is_empty());
        let spawned = std::thread::Builder::new()
            .name("lanwork-migrate".to_owned())
            .spawn(move || {
                let result = backup::migrate_data_dir(&store, &target, env_blocks, |progress| {
                    let text = backup::progress_text(progress);
                    let _ = slint::invoke_from_event_loop(move || {
                        with_settings(move |page, ui| page.show_progress(ui, &text));
                    });
                });
                let _ = slint::invoke_from_event_loop(move || {
                    with_settings(move |page, ui| page.finish_migrate(ui, result));
                });
            });
        if spawned.is_err() {
            self.busy.set(false);
            self.finish(ui, Err("不能开始迁移，仍使用原来的目录".to_owned()));
        }
    }

    fn show_progress(&self, ui: &Panel, text: &str) {
        ui.global::<SettingsPageData>()
            .set_data_dir_action(text.into());
    }

    fn finish_migrate(&mut self, ui: &Panel, result: Result<std::path::PathBuf, MigrateError>) {
        self.busy.set(false);
        match result {
            Ok(dir) => {
                self.store
                    .log_info(&format!("数据目录已切换：{}", dir.display()));
                ui.set_error(self.hotkey_error().unwrap_or_default().into());
            }
            Err(err) => {
                let message = err.to_string();
                self.store.log_warn(&format!("设置页：{message}"));
                ui.set_error(message.into());
            }
        }
        self.push(ui);
    }

    fn arm_idle(&self, ui: &Panel) {
        let data = ui.global::<SettingsPageData>();
        data.set_data_dir_action("更换".into());
        data.set_data_dir_enabled(true);
        data.set_backup_action("立即备份".into());
        data.set_backup_enabled(true);
        data.set_export_action("导出".into());
        data.set_export_enabled(true);
        let confirming = self.pending_import.borrow().is_some();
        data.set_import_confirming(confirming);
        data.set_import_overview(self.import_overview.borrow().as_str().into());
        data.set_import_action(if confirming { "确认导入" } else { "导入" }.into());
        data.set_import_enabled(true);
    }

    fn hold(&self, ui: &Panel, kind: Hold) {
        self.busy.set(true);
        let data = ui.global::<SettingsPageData>();
        data.set_data_dir_enabled(false);
        data.set_backup_enabled(false);
        data.set_export_enabled(false);
        data.set_import_enabled(false);
        match kind {
            Hold::Migrate(text) => data.set_data_dir_action(text.into()),
            Hold::Backup => data.set_backup_action("正在备份".into()),
            Hold::Export => data.set_export_action("正在导出".into()),
            Hold::Read => data.set_import_action("正在读取".into()),
            Hold::Import => data.set_import_action("正在导入".into()),
        }
    }

    fn push_backups(&self, ui: &Panel) {
        let listed = match BackupCommands::open(self.store.clone()).list_retained() {
            Ok(listed) => listed,
            Err(err) => {
                let message = err.to_string();
                self.store.log_warn(&format!("设置页：{message}"));
                ui.set_error(message.into());
                Vec::new()
            }
        };
        let rows: Vec<SettingsBackup> = listed
            .iter()
            .map(|item| SettingsBackup {
                label: item.label().into(),
            })
            .collect();
        ui.global::<SettingsPageData>()
            .set_backups(ModelRc::new(VecModel::from(rows)));
    }

    fn backup_now(&mut self, ui: &Panel) {
        if self.busy.get() {
            return;
        }
        self.cancel_recording(ui);
        self.hold(ui, Hold::Backup);
        let store = self.store.clone();
        let now = crate::host::unix_now_ms();
        let spawned = std::thread::Builder::new()
            .name("lanwork-backup".to_owned())
            .spawn(move || {
                let result = BackupCommands::open(store.clone())
                    .backup_manual(now)
                    .map(|path| {
                        store.log_info(&format!("设置页：已手动备份：{}", path.display()));
                    })
                    .map_err(|err| err.to_string());
                let _ = slint::invoke_from_event_loop(move || {
                    with_settings(move |page, ui| page.finish_transfer(ui, result));
                });
            });
        if spawned.is_err() {
            self.busy.set(false);
            self.finish(ui, Err("不能开始备份".to_owned()));
        }
    }

    fn export_clicked(&mut self, ui: &Panel) {
        if self.busy.get() {
            return;
        }
        self.cancel_recording(ui);
        let Some(hwnd) = crate::bar_win::hwnd_of(ui.window()) else {
            self.finish(ui, Err("不能选择文件".to_owned()));
            return;
        };
        match crate::file_pick::pick_export(hwnd) {
            Ok(None) => {}
            Ok(Some(path)) => self.begin_export(ui, path),
            Err(message) => self.finish(ui, Err(message)),
        }
    }

    fn begin_export(&mut self, ui: &Panel, dest: PathBuf) {
        self.hold(ui, Hold::Export);
        let store = self.store.clone();
        let now = crate::host::unix_now_ms();
        let spawned = std::thread::Builder::new()
            .name("lanwork-export".to_owned())
            .spawn(move || {
                let result = BackupCommands::open(store.clone())
                    .export(&dest, now)
                    .map(|_| {
                        store.log_info(&format!("设置页：已导出：{}", dest.display()));
                    })
                    .map_err(|err| err.to_string());
                let _ = slint::invoke_from_event_loop(move || {
                    with_settings(move |page, ui| page.finish_transfer(ui, result));
                });
            });
        if spawned.is_err() {
            self.busy.set(false);
            self.finish(ui, Err("不能开始导出".to_owned()));
        }
    }

    fn import_clicked(&mut self, ui: &Panel) {
        if self.busy.get() {
            return;
        }
        self.cancel_recording(ui);
        let Some(hwnd) = crate::bar_win::hwnd_of(ui.window()) else {
            self.finish(ui, Err("不能选择文件".to_owned()));
            return;
        };
        match crate::file_pick::pick_import(hwnd) {
            Ok(None) => {}
            Ok(Some(path)) => self.begin_inspect(ui, path),
            Err(message) => self.finish(ui, Err(message)),
        }
    }

    fn begin_inspect(&mut self, ui: &Panel, path: PathBuf) {
        self.hold(ui, Hold::Read);
        let store = self.store.clone();
        let spawned = std::thread::Builder::new()
            .name("lanwork-import".to_owned())
            .spawn(move || {
                let result = BackupCommands::open(store)
                    .inspect(&path)
                    .map(|overview| overview_text(&overview))
                    .map_err(|err| err.to_string());
                let _ = slint::invoke_from_event_loop(move || {
                    with_settings(move |page, ui| page.finish_inspect(ui, path, result));
                });
            });
        if spawned.is_err() {
            self.busy.set(false);
            self.finish(ui, Err("不能开始导入".to_owned()));
        }
    }

    fn finish_inspect(&mut self, ui: &Panel, path: PathBuf, result: Result<String, String>) {
        self.busy.set(false);
        match result {
            Ok(overview) => {
                *self.pending_import.borrow_mut() = Some(path);
                *self.import_overview.borrow_mut() = overview;
                ui.set_error(self.hotkey_error().unwrap_or_default().into());
                self.push(ui);
            }
            Err(message) => {
                self.pending_import.borrow_mut().take();
                self.import_overview.borrow_mut().clear();
                self.finish(ui, Err(message));
            }
        }
    }

    fn confirm_import(&mut self, ui: &Panel) {
        if self.busy.get() {
            return;
        }
        let Some(path) = self.pending_import.borrow().clone() else {
            self.push(ui);
            return;
        };
        self.cancel_recording(ui);
        self.hold(ui, Hold::Import);
        let store = self.store.clone();
        let apps = self.apps.clone();
        let now = crate::host::unix_now_ms();
        let spawned = std::thread::Builder::new()
            .name("lanwork-import".to_owned())
            .spawn(move || {
                let outcome = BackupCommands::open(store).import(&path, now);
                let catalog_error = if outcome.is_ok() {
                    apps.as_ref()
                        .and_then(|apps| apps.reload_user_catalog().err())
                } else {
                    None
                };
                let result = outcome.map(|_| ()).map_err(|err| err.to_string());
                let _ = slint::invoke_from_event_loop(move || {
                    with_settings(move |page, ui| page.finish_import(ui, result, catalog_error));
                });
            });
        if spawned.is_err() {
            self.busy.set(false);
            self.finish(ui, Err("不能开始导入".to_owned()));
        }
    }

    fn finish_import(
        &mut self,
        ui: &Panel,
        result: Result<(), String>,
        catalog_error: Option<String>,
    ) {
        self.busy.set(false);
        if let Err(message) = result {
            self.finish(ui, Err(message));
            return;
        }
        self.pending_import.borrow_mut().take();
        self.import_overview.borrow_mut().clear();
        self.store.log_info("设置页：导入完成");
        let applied = self.apply_imported_runtime();
        let result = match (applied, catalog_error) {
            (Ok(()), Some(message)) => {
                self.store.log_warn(&format!("设置页：{message}"));
                Err(message)
            }
            (Err(message), Some(catalog)) => {
                self.store.log_warn(&format!("设置页：{catalog}"));
                Err(message)
            }
            (result, None) => result,
        };
        self.finish(ui, result);
    }

    fn cancel_import(&mut self, ui: &Panel) {
        if self.busy.get() {
            return;
        }
        self.pending_import.borrow_mut().take();
        self.import_overview.borrow_mut().clear();
        ui.set_error(self.hotkey_error().unwrap_or_default().into());
        self.push(ui);
    }

    fn finish_transfer(&mut self, ui: &Panel, result: Result<(), String>) {
        self.busy.set(false);
        self.finish(ui, result);
    }

    /// 导入已经换过磁盘和已打开的服务内存。这里让正在跑的界面跟上那份配置。
    fn apply_imported_runtime(&mut self) -> Result<(), String> {
        crate::host::refresh_theme(&self.commands, &self.store);
        crate::hotcorner::apply(self.commands.current().hot_corner);
        if let Some(apps) = &self.apps {
            apps.set_web_search_engine(self.commands.current().web_search_engine);
        }
        crate::searchbar::requery();
        let hotkey_gap = match self.hotkeys.register(self.commands.clone()) {
            Some(unregistered) => {
                for role in &unregistered {
                    self.store
                        .log_warn(&format!("设置页：{}没有注册上：热键已被占用", role.title()));
                }
                self.recording.note_unregistered(unregistered);
                None
            }
            None => {
                self.store
                    .log_warn("设置页：热键服务没有应答，热键没有按导入的配置注册");
                Some("热键服务没有应答".to_owned())
            }
        };
        let executable = std::env::current_exe().map_err(|_| "读不到可执行文件路径".to_owned())?;
        let startup =
            crate::registry::apply_startup(self.commands.current().launch_at_startup, &executable)
                .map_err(|err| startup_error_text(&err));
        match (startup, hotkey_gap) {
            (Err(message), _) => Err(message),
            (Ok(()), Some(message)) => Err(message),
            (Ok(()), None) => Ok(()),
        }
    }

    fn push_hidden(&self, ui: &Panel) {
        let listed = self
            .apps
            .as_ref()
            .map(|apps| apps.hidden_apps())
            .unwrap_or_default();
        let names: Vec<SettingsHiddenApp> = listed
            .iter()
            .map(|app| SettingsHiddenApp {
                name: app.name.as_str().into(),
            })
            .collect();
        *self.hidden_keys.borrow_mut() = listed.into_iter().map(|app| app.launch_key).collect();
        ui.global::<SettingsPageData>()
            .set_hidden_apps(ModelRc::new(VecModel::from(names)));
    }

    fn restore_hidden(&mut self, ui: &Panel, index: i32) {
        self.cancel_recording(ui);
        let Some(apps) = self.apps.clone() else {
            self.push(ui);
            return;
        };
        let Some(key) = usize::try_from(index)
            .ok()
            .and_then(|index| self.hidden_keys.borrow().get(index).cloned())
        else {
            self.push(ui);
            return;
        };
        let result = apps.restore_hidden_app(&key);
        if result.is_ok() {
            crate::searchbar::requery();
        }
        self.finish(ui, result);
    }

    /// 搜索条隐藏了一项之后刷新这份列表。
    pub(crate) fn refresh_hidden(&self, ui: &Panel) {
        self.push_hidden(ui);
    }

    fn fail(&self, ui: &Panel, message: &str) {
        self.store.log_warn(&format!("设置页：{message}"));
        ui.set_error(message.into());
    }

    /// 一次操作的结果：失败显示原因；成功时清掉错误，但没注册上的热键提示仍然留着。
    /// 两种情况都按配置里的值重推界面。
    fn finish(&self, ui: &Panel, result: Result<(), String>) {
        match result {
            Ok(()) => ui.set_error(self.hotkey_error().unwrap_or_default().into()),
            Err(message) => self.fail(ui, &message),
        }
        self.push(ui);
    }

    fn set_launch_at_startup(&mut self, ui: &Panel, enabled: bool) {
        self.cancel_recording(ui);
        let result = change_launch_at_startup(&self.commands, enabled, |value| {
            let executable =
                std::env::current_exe().map_err(|_| "读不到可执行文件路径".to_owned())?;
            apply_startup(value, &executable)
        });
        let result = match result {
            Ok(change) => match change.registry_error {
                Some(reason) => Err(startup_error_text(&reason)),
                None => Ok(()),
            },
            Err(err) => Err(err.to_string()),
        };
        self.finish(ui, result);
    }

    fn choose_corner(&mut self, ui: &Panel, index: i32) {
        self.cancel_recording(ui);
        let Some(corner) = hot_corner_at(index) else {
            self.push(ui);
            return;
        };
        let result = change_hot_corner(&self.commands, corner, crate::hotcorner::apply)
            .map(|_| ())
            .map_err(|err| err.to_string());
        self.finish(ui, result);
    }

    fn choose_theme(&mut self, ui: &Panel, index: i32) {
        self.cancel_recording(ui);
        let Some(theme) = theme_at(index) else {
            self.push(ui);
            return;
        };
        let result = change_theme(&self.commands, theme, |_| {
            crate::host::refresh_theme(&self.commands, &self.store);
        })
        .map(|_| ())
        .map_err(|err| err.to_string());
        self.finish(ui, result);
    }

    fn choose_engine(&mut self, ui: &Panel, index: i32) {
        self.cancel_recording(ui);
        let Some(engine) = web_engine_at(index) else {
            self.push(ui);
            return;
        };
        let apps = self.apps.clone();
        let result = change_web_search_engine(&self.commands, engine, |engine| {
            if let Some(apps) = &apps {
                apps.set_web_search_engine(engine);
            }
            crate::searchbar::requery();
        })
        .map(|_| ())
        .map_err(|err| err.to_string());
        self.finish(ui, result);
    }

    fn set_quiet(&mut self, ui: &Panel, enabled: bool) {
        self.cancel_recording(ui);
        let hours = if enabled {
            Some(
                self.commands
                    .current()
                    .quiet_hours
                    .unwrap_or_else(default_quiet_hours),
            )
        } else {
            None
        };
        self.finish(ui, self.write_quiet(hours));
    }

    fn commit_quiet_start(&mut self, ui: &Panel, text: &str) {
        self.cancel_recording(ui);
        let Some(start) = parse_clock(text) else {
            self.finish(ui, Err(CLOCK_FORMAT_ERROR.to_owned()));
            return;
        };
        let end = self
            .commands
            .current()
            .quiet_hours
            .map(|hours| hours.end)
            .unwrap_or_else(|| default_quiet_hours().end);
        self.finish(ui, self.write_quiet(Some(QuietHours { start, end })));
    }

    fn commit_quiet_end(&mut self, ui: &Panel, text: &str) {
        self.cancel_recording(ui);
        let Some(end) = parse_clock(text) else {
            self.finish(ui, Err(CLOCK_FORMAT_ERROR.to_owned()));
            return;
        };
        let start = self
            .commands
            .current()
            .quiet_hours
            .map(|hours| hours.start)
            .unwrap_or_else(|| default_quiet_hours().start);
        self.finish(ui, self.write_quiet(Some(QuietHours { start, end })));
    }

    fn write_quiet(&self, hours: Option<QuietHours>) -> Result<(), String> {
        change_quiet_hours(&self.commands, hours)
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    fn commit_snooze(&mut self, ui: &Panel, text: &str) {
        self.cancel_recording(ui);
        let Some(days) = parse_snooze_days(text) else {
            self.finish(ui, Err(SNOOZE_DAYS_ERROR.to_owned()));
            return;
        };
        let result = change_snooze_days(&self.commands, days)
            .map(|_| ())
            .map_err(|err| err.to_string());
        self.finish(ui, result);
    }

    /// 离开「常规」时放弃正在录入的热键。分类本身不写入配置。
    fn choose_section(&mut self, ui: &Panel, index: i32) {
        if index != 0 {
            self.cancel_recording(ui);
        }
    }

    /// 点热键框：开始录入；录入中再点一次放弃。
    fn toggle_recording(&mut self, ui: &Panel) {
        if self.recording.is_active() {
            self.cancel_recording(ui);
            return;
        }
        if !self.capture_ready {
            self.fail(ui, CAPTURE_UNAVAILABLE);
            return;
        }
        ui.set_error(self.hotkey_error().unwrap_or_default().into());
        self.recording.begin();
        if let Err(message) = self.hotkeys.suspend() {
            // 平台线程没应答，那次卸载任务可能迟一点才执行，所以也要走出口把热键注册回去。
            self.end_recording(ui, EndReason::Abandon);
            self.fail(ui, &message);
            return;
        }
        keycap::begin();
        self.push(ui);
    }
}

/// 子类在窗口过程里收到录入结果。排到下一轮事件循环再处理，不在窗口过程里写盘和改界面。
pub(crate) fn on_recorded(recorded: Recorded) {
    slint::Timer::single_shot(Duration::ZERO, move || {
        with_settings(move |page, ui| page.end_recording(ui, EndReason::Recorded(recorded)));
    });
}
