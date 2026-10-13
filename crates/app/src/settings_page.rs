//! 面板的设置页：开机启动、热角、搜索条热键、关于与更新，以及隐藏的应用。
//!
//! 决定在 `lanwork_core::panel` 和 `lanwork_core::shell`：先写 `config.json`，成功后才写注册表、
//! 换热角；热键先注册新的、再写盘、再卸旧的。这里只把结果推给界面，并在失败时用配置里的原值重推，
//! 让控件回到原状。错误显示在面板底部的错误条，与其他标签一致。
//! 录入热键时已注册的热键由平台线程卸掉，按键由 `keycap` 在窗口上拦截。
//! 「录入中」只有一个出口 [`SettingsPage::end_recording`]：录到组合、Esc、再点一次、失焦、收起、
//! 切标签、关闭和进程退出都走它，它把卸掉的热键注册回去。

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Duration;

use lanwork_core::config::ConfigCommands;
use lanwork_core::dispatch::Dispatch;
use lanwork_core::panel::{
    change_hot_corner, change_launch_at_startup, hot_corner_at, hot_corner_titles, settings_rows,
    startup_error_text,
};
use lanwork_core::shell::{EndReason, HotkeyRecording, HotkeyRole, Recorded};
use lanwork_core::storage::Store;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::keycap;
use crate::panel::with_settings;
use crate::platform::HotkeyControl;
use crate::registry::apply_startup;
use crate::{Panel, SettingsHiddenApp, SettingsPageData};

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
}

impl SettingsPage {
    pub(crate) fn new(ui: &Panel, services: SettingsServices, store: Store) -> Self {
        let titles: Vec<SharedString> = hot_corner_titles().into_iter().map(Into::into).collect();
        let data = ui.global::<SettingsPageData>();
        data.set_corner_titles(ModelRc::from(std::rc::Rc::new(VecModel::from(titles))));
        data.on_set_launch_at_startup(|enabled| {
            with_settings(move |page, ui| page.set_launch_at_startup(ui, enabled));
        });
        data.on_choose_corner(|index| with_settings(move |page, ui| page.choose_corner(ui, index)));
        data.on_toggle_recording(|| with_settings(SettingsPage::toggle_recording));
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
        self.push_hidden(ui);
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
