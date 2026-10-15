//! 设置页的布局和点击检查。用软件渲染的窗口把 `Panel` 的设置标签摆出来，点控件，
//! 确认它们各自发出请求，并且控件只显示 Rust 推来的值，不自己翻转。
//! 坐标与 `settingspage.slint` 开头的注释一致。

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
const PAGE_Y: f32 = 49.0;
const NAV_X: f32 = 40.0;
const CONTROL_X: f32 = 132.0 + 16.0 + 120.0;

fn open(dark: bool) -> (Panel, Rc<MinimalSoftwareWindow>) {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    window.set_size(slint::PhysicalSize::new(WIDTH, HEIGHT));
    let ui = Panel::new().expect("panel");
    ui.set_dark(dark);
    ui.set_tab(3);
    let data = ui.global::<SettingsPageData>();
    let titles = |items: &[&str]| {
        let titles: Vec<slint::SharedString> = items.iter().copied().map(Into::into).collect();
        ModelRc::new(VecModel::from(titles))
    };
    data.set_corner_titles(titles(&["右上", "左上", "左下", "右下", "关闭"]));
    data.set_theme_titles(titles(&["明", "暗", "跟随系统"]));
    data.set_engine_titles(titles(&["Google", "必应", "百度"]));
    data.set_hotkey("Ctrl+Alt+M".into());
    data.set_snooze_days("3".into());
    data.set_data_dir(r"C:\Users\me\Documents\Lanwork".into());
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

fn section_center(index: f32) -> f32 {
    PAGE_Y + 8.0 + 40.0 * index + 18.0
}

fn row_center(index: f32) -> f32 {
    PAGE_Y + 8.0 + 48.0 * index + 24.0
}

fn open_section(ui: &Panel, index: f32) {
    click(ui, NAV_X, section_center(index));
}

#[test]
fn each_control_asks_for_its_own_change_and_does_not_flip_by_itself() {
    let (ui, _window) = open(false);
    let data = ui.global::<SettingsPageData>();
    let startup = Rc::new(RefCell::new(Vec::new()));
    let corners = Rc::new(RefCell::new(Vec::new()));
    let recordings = Rc::new(Cell::new(0));
    let themes = Rc::new(RefCell::new(Vec::new()));
    let engines = Rc::new(RefCell::new(Vec::new()));
    {
        let startup = Rc::clone(&startup);
        data.on_set_launch_at_startup(move |value| startup.borrow_mut().push(value));
        let corners = Rc::clone(&corners);
        data.on_choose_corner(move |index| corners.borrow_mut().push(index));
        let recordings = Rc::clone(&recordings);
        data.on_toggle_recording(move || recordings.set(recordings.get() + 1));
        let themes = Rc::clone(&themes);
        data.on_choose_theme(move |index| themes.borrow_mut().push(index));
        let engines = Rc::clone(&engines);
        data.on_choose_engine(move |index| engines.borrow_mut().push(index));
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

    // 「明」大约 33 逻辑像素宽，点第二项「暗」。
    click(&ui, CONTROL_X + 33.0 + 16.0, row_center(3.0));
    assert_eq!(*themes.borrow(), [1]);
    assert_eq!(data.get_theme(), 0, "主题不自己换");

    // 「Google」大约 62 逻辑像素宽，点第二项「必应」。
    click(&ui, CONTROL_X + 62.0 + 16.0, row_center(4.0));
    assert_eq!(*engines.borrow(), [1]);
    assert_eq!(data.get_engine(), 0, "搜索引擎不自己换");
}

#[test]
fn quiet_hours_and_the_other_sections_keep_their_own_controls() {
    let (ui, _window) = open(false);
    let data = ui.global::<SettingsPageData>();
    let quiet = Rc::new(RefCell::new(Vec::new()));
    let startup = Rc::new(RefCell::new(Vec::new()));
    let sections = Rc::new(RefCell::new(Vec::new()));
    {
        let quiet = Rc::clone(&quiet);
        data.on_set_quiet(move |value| quiet.borrow_mut().push(value));
        let startup = Rc::clone(&startup);
        data.on_set_launch_at_startup(move |value| startup.borrow_mut().push(value));
        let sections = Rc::clone(&sections);
        data.on_choose_section(move |index| sections.borrow_mut().push(index));
    }

    open_section(&ui, 1.0);
    assert_eq!(*sections.borrow(), [1]);
    click(&ui, CONTROL_X + 20.0, row_center(0.0));
    click(&ui, CONTROL_X + 20.0, row_center(0.0));
    assert_eq!(*quiet.borrow(), [true, true]);
    assert!(!data.get_quiet_on(), "安静时段不自己打开");
    assert!(startup.borrow().is_empty(), "待办页点不到开机启动");

    data.set_quiet_on(true);
    data.set_quiet_start("22:00".into());
    data.set_quiet_end("08:00".into());
    slint::platform::update_timers_and_animations();
    click(&ui, CONTROL_X + 20.0, row_center(0.0));
    assert_eq!(quiet.borrow().last(), Some(&false));

    open_section(&ui, 2.0);
    click(&ui, CONTROL_X + 20.0, row_center(0.0));
    assert_eq!(quiet.borrow().len(), 3, "数据页没有安静时段开关");
    assert!(startup.borrow().is_empty());
}

#[test]
fn the_data_directory_button_asks_to_choose_and_does_not_change_the_path() {
    let (ui, _window) = open(false);
    let data = ui.global::<SettingsPageData>();
    let chooses = Rc::new(Cell::new(0));
    {
        let chooses = Rc::clone(&chooses);
        data.on_choose_data_dir(move || chooses.set(chooses.get() + 1));
    }
    open_section(&ui, 2.0);
    let path = data.get_data_dir();
    // 行贴右。「更换」大约 52 逻辑像素宽，点它的中间。
    click(&ui, WIDTH as f32 - 16.0 - 26.0, row_center(0.0));
    assert_eq!(chooses.get(), 1);
    assert_eq!(data.get_data_dir(), path, "点更换不自己改路径");
    data.set_data_dir_enabled(false);
    click(&ui, WIDTH as f32 - 16.0 - 26.0, row_center(0.0));
    assert_eq!(chooses.get(), 1, "迁移进行中不再发出请求");
}

#[test]
fn the_update_button_sits_on_the_other_section() {
    let (ui, _window) = open(false);
    let data = ui.global::<SettingsPageData>();
    let clicks = Rc::new(Cell::new(0));
    {
        let clicks = Rc::clone(&clicks);
        data.on_update_clicked(move || clicks.set(clicks.get() + 1));
    }
    assert_eq!(data.get_update_button().as_str(), "检查更新");
    // 右栏上边距 8 + 按钮 y 52 + 半高 15。
    let y = PAGE_Y + 8.0 + 52.0 + 15.0;
    click(&ui, CONTROL_X + 16.0, y);
    assert_eq!(clicks.get(), 0, "常规页没有检查更新");
    open_section(&ui, 3.0);
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
    open_section(&ui, 3.0);
    // 上边距 8、更新块 88、节标题 40，然后第一条右侧的「恢复」。
    let y = PAGE_Y + 8.0 + 88.0 + 40.0 + 22.0;
    click(&ui, WIDTH as f32 - 16.0 - 24.0, y);
    assert_eq!(*restored.borrow(), [0]);
}
