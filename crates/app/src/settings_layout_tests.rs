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
    // 三行设置、节标题，然后第一条隐藏项右侧的「恢复」。
    let y = TITLE + 1.0 + PAD_TOP + ROW * 3.0 + 40.0 + 22.0;
    click(&ui, WIDTH as f32 - 24.0 - 16.0, y);
    assert_eq!(*restored.borrow(), [0]);
}
