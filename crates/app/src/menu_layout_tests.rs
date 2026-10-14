//! 搜索条应用菜单的点击检查。用软件渲染把 `SearchBar` 摆出来：右键打开菜单，
//! 悬停高亮，单击执行，不可用项点了不执行，菜单外点击和 Esc 关掉菜单且不打开底下的结果。
//! Alt+数字的去向在 `lanwork_core::shell::alt_route`，这里不重复。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{BarMenuItem, BarRow, SearchBar};

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

const INPUT: f32 = 56.0;
const ROW_Y: f32 = 6.0;
const ROW_H: f32 = 48.0;
const MENU_ITEM: f32 = 36.0;
const MENU_PAD: f32 = 4.0;
const MENU_W: f32 = 272.0;

fn sample_row() -> BarRow {
    BarRow {
        label_head: "记事本".into(),
        label_match: String::new().into(),
        label_tail: String::new().into(),
        kind: "应用".into(),
        location: String::new().into(),
        location_always: false,
        icon: slint::Image::default(),
        has_icon: false,
        draw_glyph: true,
        glyph: 0,
        y: ROW_Y,
        group_start: false,
    }
}

fn menu_items() -> Vec<BarMenuItem> {
    [
        ("以其他用户身份运行", "Alt+1", true),
        ("以管理员身份运行", "Alt+2", true),
        ("打开文件所在文件夹", "Alt+3", true),
        ("打开目标文件夹", "Alt+4", false),
        ("隐藏", "Alt+5", true),
    ]
    .into_iter()
    .map(|(label, hint, enabled)| BarMenuItem {
        label: label.into(),
        hint: hint.into(),
        enabled,
    })
    .collect()
}

fn open() -> (SearchBar, Rc<MinimalSoftwareWindow>) {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    window.set_size(slint::PhysicalSize::new(680, 120));
    let ui = SearchBar::new().expect("search bar");
    ui.set_rows(ModelRc::new(VecModel::from(vec![sample_row()])));
    ui.set_list_content_height(ROW_Y + ROW_H + ROW_Y);
    ui.set_selected(0);
    ui.show().expect("show");
    slint::platform::update_timers_and_animations();
    (ui, window)
}

fn pointer(ui: &SearchBar, kind: PointerKind, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    let event = match kind {
        PointerKind::Move => WindowEvent::PointerMoved { position },
        PointerKind::Down(button) => WindowEvent::PointerPressed { position, button },
        PointerKind::Up(button) => WindowEvent::PointerReleased { position, button },
    };
    ui.window().dispatch_event(event);
}

enum PointerKind {
    Move,
    Down(PointerEventButton),
    Up(PointerEventButton),
}

fn click(ui: &SearchBar, button: PointerEventButton, x: f32, y: f32) {
    pointer(ui, PointerKind::Move, x, y);
    pointer(ui, PointerKind::Down(button), x, y);
    pointer(ui, PointerKind::Up(button), x, y);
}

fn row_point() -> (f32, f32) {
    (120.0, INPUT + 1.0 + ROW_Y + ROW_H / 2.0)
}

fn show_menu(ui: &SearchBar, x: f32, y: f32) {
    ui.set_menu_items(ModelRc::new(VecModel::from(menu_items())));
    ui.set_menu_anchor_x(x);
    ui.set_menu_anchor_y(y);
    ui.set_menu_open(true);
    slint::platform::update_timers_and_animations();
}

fn item_center(anchor_x: f32, anchor_y: f32, index: f32) -> (f32, f32) {
    let x = anchor_x.clamp(8.0, 680.0 - MENU_W - 8.0);
    let y = anchor_y.max(8.0);
    (
        x + MENU_PAD + (MENU_W - MENU_PAD * 2.0) / 2.0,
        y + MENU_PAD + index * MENU_ITEM + MENU_ITEM / 2.0,
    )
}

#[test]
fn right_click_opens_the_menu_and_clicks_follow_the_pointer() {
    let (ui, _window) = open();
    let opened = Rc::new(RefCell::new(None));
    let activated = Rc::new(RefCell::new(Vec::new()));
    let escapes = Rc::new(Cell::new(0));
    let row_hits = Rc::new(Cell::new(0));
    {
        let opened = Rc::clone(&opened);
        ui.on_open_menu(move |index, x, y| {
            *opened.borrow_mut() = Some((index, x, y));
        });
        let activated = Rc::clone(&activated);
        ui.on_menu_activate(move |index| activated.borrow_mut().push(index));
        let escapes = Rc::clone(&escapes);
        ui.on_escape(move || escapes.set(escapes.get() + 1));
        let row_hits = Rc::clone(&row_hits);
        ui.on_activate(move |_| row_hits.set(row_hits.get() + 1));
    }

    let (x, y) = row_point();
    click(&ui, PointerEventButton::Right, x, y);
    let (index, anchor_x, anchor_y) = opened.borrow().expect("右键打开菜单");
    assert_eq!(index, 0);
    assert!((anchor_x - x).abs() < 1.0, "anchor x {anchor_x}");
    assert!((anchor_y - y).abs() < 1.0, "anchor y {anchor_y}");
    assert_eq!(row_hits.get(), 0, "右键不打开结果");

    show_menu(&ui, anchor_x, anchor_y);
    assert!(ui.get_menu_open());
    let (item_x, item_y) = item_center(anchor_x, anchor_y, 0.0);
    pointer(&ui, PointerKind::Move, item_x, item_y);
    assert_eq!(ui.get_menu_hover(), 0, "悬停高亮第一项");

    click(&ui, PointerEventButton::Left, item_x, item_y);
    assert_eq!(*activated.borrow(), [0]);
    assert!(!ui.get_menu_open(), "单击后菜单关闭");
    assert_eq!(row_hits.get(), 0);

    show_menu(&ui, anchor_x, anchor_y);
    let (disabled_x, disabled_y) = item_center(anchor_x, anchor_y, 3.0);
    pointer(&ui, PointerKind::Move, disabled_x, disabled_y);
    assert_eq!(ui.get_menu_hover(), -1, "不可用项不高亮");
    click(&ui, PointerEventButton::Left, disabled_x, disabled_y);
    assert_eq!(activated.borrow().len(), 1, "不可用项不执行");
    assert!(ui.get_menu_open(), "点不可用项不关闭菜单");

    click(&ui, PointerEventButton::Left, 16.0, 16.0);
    assert!(!ui.get_menu_open(), "菜单外点击关闭");
    assert_eq!(row_hits.get(), 0, "这次点击不打开底下的结果");
    assert_eq!(escapes.get(), 0);

    show_menu(&ui, anchor_x, anchor_y);
    ui.invoke_focus_input();
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: "\u{001b}".into(),
    });
    assert!(!ui.get_menu_open(), "Esc 关闭菜单");
    assert_eq!(escapes.get(), 0, "关菜单时不收起搜索条");
}
