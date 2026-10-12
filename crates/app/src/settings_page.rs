//! 面板的设置页：开机启动、热角、搜索条热键。
//!
//! 决定在 `lanwork_core::panel` 和 `lanwork_core::shell`：先写 `config.json`，成功后才写注册表、
//! 换热角；热键先注册新的、再写盘、再卸旧的。这里只把结果推给界面，并在失败时用配置里的原值重推，
//! 让控件回到原状。错误显示在面板底部的错误条，与其他标签一致。
//! 录入热键时已注册的热键由平台线程卸掉，按键由 `keycap` 在窗口上拦截。

use std::time::Duration;

use lanwork_core::config::ConfigCommands;
use lanwork_core::panel::{
    change_hot_corner, change_launch_at_startup, hot_corner_at, hot_corner_titles, settings_rows,
    startup_error_text,
};
use lanwork_core::shell::Recorded;
use lanwork_core::storage::Store;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::keycap;
use crate::panel::with_settings;
use crate::platform::HotkeyControl;
use crate::registry::apply_startup;
use crate::{Panel, SettingsPageData};

const INVALID_HOTKEY: &str = "热键无效";
const CAPTURE_UNAVAILABLE: &str = "热键录入不可用";

/// 设置页要用的服务：配置，和平台线程上的热键注册。
pub(crate) struct SettingsServices {
    pub(crate) config: ConfigCommands,
    pub(crate) hotkeys: HotkeyControl,
}

pub(crate) struct SettingsPage {
    commands: ConfigCommands,
    hotkeys: HotkeyControl,
    store: Store,
    recording: bool,
    capture_ready: bool,
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
        let page = Self {
            commands: services.config,
            hotkeys: services.hotkeys,
            store,
            recording: false,
            capture_ready: false,
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

    /// 离开设置标签、面板收起时调用。正在录入就放弃，并把热键注册回去。
    pub(crate) fn cancel_recording(&mut self, ui: &Panel) {
        keycap::reset();
        if !self.recording {
            return;
        }
        self.recording = false;
        let result = self.hotkeys.resume_and_save(self.commands.clone(), None);
        self.finish(ui, result);
    }

    fn push(&self, ui: &Panel) {
        let rows = settings_rows(&self.commands.current());
        let data = ui.global::<SettingsPageData>();
        data.set_launch_at_startup(rows.launch_at_startup);
        data.set_corner(i32::try_from(rows.hot_corner).unwrap_or(0));
        data.set_hotkey(rows.search_hotkey.into());
        data.set_recording(self.recording);
    }

    fn fail(&self, ui: &Panel, message: &str) {
        self.store.log_warn(&format!("设置页：{message}"));
        ui.set_error(message.into());
    }

    /// 一次操作的结果：成功清掉错误，失败显示原因；两种情况都按配置里的值重推界面。
    fn finish(&self, ui: &Panel, result: Result<(), String>) {
        match result {
            Ok(()) => ui.set_error(SharedString::new()),
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
        if self.recording {
            self.cancel_recording(ui);
            return;
        }
        if !self.capture_ready {
            self.fail(ui, CAPTURE_UNAVAILABLE);
            return;
        }
        ui.set_error(SharedString::new());
        if let Err(message) = self.hotkeys.suspend() {
            self.fail(ui, &message);
            return;
        }
        keycap::begin();
        self.recording = true;
        self.push(ui);
    }

    /// 录入结束：先把卸掉的热键注册回去，再保存录到的组合。
    fn recorded(&mut self, ui: &Panel, recorded: Recorded) {
        if !self.recording {
            return;
        }
        self.recording = false;
        let (text, rejected) = match recorded {
            Recorded::Captured(hotkey) => (Some(hotkey.canonical()), false),
            Recorded::Cancelled => (None, false),
            Recorded::Rejected => (None, true),
        };
        let result = self.hotkeys.resume_and_save(self.commands.clone(), text);
        let result = match result {
            Ok(()) if rejected => Err(INVALID_HOTKEY.to_owned()),
            other => other,
        };
        self.finish(ui, result);
    }
}

/// 子类在窗口过程里收到录入结果。排到下一轮事件循环再处理，不在窗口过程里写盘和改界面。
pub(crate) fn on_recorded(recorded: Recorded) {
    slint::Timer::single_shot(Duration::ZERO, move || {
        with_settings(move |page, ui| page.recorded(ui, recorded));
    });
}
