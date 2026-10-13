//! 设置页的布局和点击检查。用软件渲染的窗口把 `Panel` 的设置标签摆出来，点三个控件，
//! 确认它们各自发出请求，并且控件只显示 Rust 推来的值，不自己翻转。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::software_renderer::RepaintBufferType;
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{Panel, SettingsHiddenApp, SettingsPageData};

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

const WIDTH: u32 = 760;
const HEIGHT: u32 = 560;
const TITLE: f32 = 48.0;
const ROW: f32 = 48.0;
const PAD_TOP: f32 = 8.0;
const CONTROL_X: f32 = 24.0 + 140.0;

fn open(dark: bool) -> (Panel, Rc<MinimalSoftwareWindow>) {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    window.set_size(slint::PhysicalSize::new(WIDTH, HEIGHT));
    let ui = Panel::new().expect("panel");
    ui.set_dark(dark);
    ui.set_tab(3);
    let data = ui.global::<SettingsPageData>();
    let titles: Vec<slint::SharedString> = ["右上", "左上", "左下", "右下", "关闭"]
        .into_iter()
        .map(Into::into)
        .collect();
    data.set_corner_titles(ModelRc::new(VecModel::from(titles)));
    data.set_hotkey("Ctrl+Alt+M".into());
    ui.show().expect("show");
    slint::platform::update_timers_and_animations();
    (ui, window)
}

fn click(ui: &Panel, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

fn row_center(index: f32) -> f32 {
    TITLE + 1.0 + PAD_TOP + ROW * index + ROW / 2.0
}

#[test]
fn each_control_asks_for_its_own_change_and_does_not_flip_by_itself() {
    let (ui, _window) = open(false);
    let data = ui.global::<SettingsPageData>();
    let startup = Rc::new(RefCell::new(Vec::new()));
    let corners = Rc::new(RefCell::new(Vec::new()));
    let recordings = Rc::new(Cell::new(0));
    {
        let startup = Rc::clone(&startup);
        data.on_set_launch_at_startup(move |value| startup.borrow_mut().push(value));
        let corners = Rc::clone(&corners);
        data.on_choose_corner(move |index| corners.borrow_mut().push(index));
        let recordings = Rc::clone(&recordings);
        data.on_toggle_recording(move || recordings.set(recordings.get() + 1));
    }

    click(&ui, CONTROL_X + 20.0, row_center(0.0));
    click(&ui, CONTROL_X + 20.0, row_center(0.0));
    assert_eq!(
        *startup.borrow(),
        [true, true],
        "开关请求相反的值，值没变就还是同一个请求"
    );
    assert!(!data.get_launch_at_startup(), "开关不自己翻转");

    data.set_launch_at_startup(true);
    click(&ui, CONTROL_X + 20.0, row_center(0.0));
    assert_eq!(startup.borrow().last(), Some(&false));

    click(&ui, CONTROL_X + 48.0 + 24.0, row_center(1.0));
    assert_eq!(*corners.borrow(), [1], "第二项是左上");
    assert_eq!(data.get_corner(), 0, "热角不自己换");

    click(&ui, CONTROL_X + 90.0, row_center(2.0));
    assert_eq!(recordings.get(), 1);
}

#[test]
fn the_update_button_asks_to_check_without_moving_the_rows_above() {
    let (ui, _window) = open(false);
    let data = ui.global::<SettingsPageData>();
    let clicks = Rc::new(Cell::new(0));
    {
        let clicks = Rc::clone(&clicks);
        data.on_update_clicked(move || clicks.set(clicks.get() + 1));
    }
    assert_eq!(data.get_update_button().as_str(), "检查更新");
    // 标题 48 + 分隔线 1 + 上边距 8 + 三行 144 + 按钮 y 52 + 半高 15。
    // 隐藏的应用在这个固定高度的更新块下面，不改变按钮位置。
    let y = TITLE + 1.0 + PAD_TOP + ROW * 3.0 + 52.0 + 15.0;
    click(&ui, CONTROL_X + 16.0, y);
    assert_eq!(clicks.get(), 1, "检查更新按钮能点到");
    data.set_update_enabled(false);
    click(&ui, CONTROL_X + 16.0, y);
    assert_eq!(clicks.get(), 1, "检查进行中不再发出请求");
}

#[test]
fn restore_asks_for_the_hidden_app_under_the_pointer() {
    let (ui, _window) = open(false);
    let data = ui.global::<SettingsPageData>();
    data.set_hidden_apps(ModelRc::new(VecModel::from(vec![SettingsHiddenApp {
        name: "记事本".into(),
    }])));
    slint::platform::update_timers_and_animations();
    let restored = Rc::new(RefCell::new(Vec::new()));
    {
        let restored = Rc::clone(&restored);
        data.on_restore_hidden(move |index| restored.borrow_mut().push(index));
    }
    // 三行、说明收起时的更新块 88、节标题 40，然后第一条右侧的「恢复」。
    let y = TITLE + 1.0 + PAD_TOP + ROW * 3.0 + 88.0 + 40.0 + 22.0;
    click(&ui, WIDTH as f32 - 24.0 - 16.0, y);
    assert_eq!(*restored.borrow(), [0]);
}
