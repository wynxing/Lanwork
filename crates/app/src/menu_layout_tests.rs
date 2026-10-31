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
fn pointer_move_reports_the_row_and_click_opens_it() {
    let (ui, window) = open();
    window.set_size(slint::PhysicalSize::new(680, 240));
    let second_y = ROW_Y + ROW_H;
    ui.set_rows(ModelRc::new(VecModel::from(vec![
        sample_row(),
        BarRow {
            y: second_y,
            ..sample_row()
        },
    ])));
    ui.set_list_content_height(second_y + ROW_H + ROW_Y);
    ui.set_selected(0);
    slint::platform::update_timers_and_animations();

    let hovers = Rc::new(RefCell::new(Vec::new()));
    let activates = Rc::new(RefCell::new(Vec::new()));
    {
        let hovers = Rc::clone(&hovers);
        ui.on_hover(move |index, x, y| hovers.borrow_mut().push((index, x, y)));
        let activates = Rc::clone(&activates);
        ui.on_activate(move |index| activates.borrow_mut().push(index));
    }

    let (x, y) = row_point();
    pointer(&ui, PointerKind::Move, x, y);
    assert!(
        hovers.borrow().is_empty(),
        "已经选中的行不因指针移动再报一次"
    );

    let second = INPUT + 1.0 + second_y + ROW_H / 2.0;
    pointer(&ui, PointerKind::Move, x, second);
    let reported = hovers.borrow().clone();
    assert_eq!(reported.len(), 1, "移到另一行时报告这一行");
    assert_eq!(reported[0].0, 1);
    assert!(
        (reported[0].1 - x).abs() < 1.0 && (reported[0].2 - second).abs() < 1.0,
        "坐标是窗口里的指针位置 {:?}，期望 ({x}, {second})",
        reported[0]
    );

    click(&ui, PointerEventButton::Left, x, second);
    assert_eq!(*activates.borrow(), [1], "单击打开点中的那一行");
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
        ui.on_menu_activate(move |index| {
            activated.borrow_mut().push(index);
            false
        });
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

fn press(ui: &SearchBar, text: &str) {
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.into() });
    ui.window()
        .dispatch_event(WindowEvent::KeyReleased { text: text.into() });
}

fn key(ui: &SearchBar, text: &str, down: bool) {
    let event = if down {
        WindowEvent::KeyPressed { text: text.into() }
    } else {
        WindowEvent::KeyReleased { text: text.into() }
    };
    ui.window().dispatch_event(event);
}

#[test]
fn right_arrow_opens_the_menu_only_when_the_caret_is_at_the_end() {
    let (ui, _window) = open();
    ui.invoke_focus_input();
    let opened = Rc::new(Cell::new(0));
    {
        let opened = Rc::clone(&opened);
        ui.on_open_selected_menu(move || opened.set(opened.get() + 1));
        let caret = ui.clone_strong();
        ui.on_caret_at_end(move |offset| {
            usize::try_from(offset).ok() == Some(caret.get_text().len())
        });
    }

    press(&ui, "a");
    press(&ui, "b");
    assert_eq!(ui.get_text().as_str(), "ab");
    press(&ui, "\u{F702}");
    press(&ui, "\u{F703}");
    assert_eq!(opened.get(), 0, "光标从中间回到末尾时只移动光标");
    press(&ui, "\u{F703}");
    assert_eq!(opened.get(), 1, "光标已经在末尾时右方向键打开菜单");

    key(&ui, "\u{0010}", true);
    key(&ui, "\u{F703}", true);
    key(&ui, "\u{F703}", false);
    key(&ui, "\u{0010}", false);
    assert_eq!(opened.get(), 1, "按着 Shift 时右方向键留给输入框");
    press(&ui, "中");
    let before = opened.get();
    press(&ui, "\u{F703}");
    assert!(
        opened.get() > before,
        "中文在末尾时按字节偏移打开菜单，文本 {:?}",
        ui.get_text()
    );
}
