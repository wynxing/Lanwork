//! 面板左侧标签。五个图标居中不动，名称是按钮旁的短提示。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use lanwork_core::panel::KEY_ESCAPE;

use crate::{NotesPageData, Panel, ShelfGroupRow, ShelfRefRow, ShelvesPageData};

thread_local! {
    static FRAME: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
}

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

const TITLES: [&str; 5] = ["待办", "便签", "收纳", "应用", "设置"];

fn open() -> Panel {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(760, 560));
    if slint::platform::set_platform(Box::new(Headless(Rc::clone(&window)))).is_ok() {
        FRAME.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&window)));
    }
    let ui = Panel::new().expect("panel");
    ui.set_tab(2);
    ui.set_tab_titles(ModelRc::new(VecModel::from(
        TITLES
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
    ui.set_panel_width(760.0);
    ui.set_panel_height(560.0);
    ui.show().expect("show");
    slint::platform::update_timers_and_animations();
    ui
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

fn center(ui: &Panel, index: i32) -> (f32, f32) {
    let x = ui.get_tab_origin_x() + ui.get_tab_hit_w() / 2.0;
    let y = ui.get_tab_origin_y() + ui.get_tab_stride() * index as f32 + ui.get_tab_hit_h() / 2.0;
    (x, y)
}

fn icon_place(ui: &Panel) -> (f32, f32) {
    (ui.get_tab_icon_x(), ui.get_tab_icon_size())
}

fn assert_icons_stay(ui: &Panel, at: (f32, f32)) {
    let (x, size) = icon_place(ui);
    let expected = ui.get_tab_origin_x() + (ui.get_tab_hit_w() - size) / 2.0;
    assert!((size - 20.0).abs() < 0.5, "图标保持 20px，实际 {size}");
    assert!(
        (x - expected).abs() < 0.5,
        "图标在热区里水平居中 {x} vs {expected}"
    );
    assert!(
        (x - at.0).abs() < 0.5 && (size - at.1).abs() < 0.5,
        "图标相对静止时没有位移 {x},{size} vs {},{}",
        at.0,
        at.1
    );
    assert!(
        (ui.get_shelf_zone_x() - 64.0).abs() < 0.5,
        "提示不改变内容区起点"
    );
}

fn assert_tip_inside(ui: &Panel) {
    let x = ui.get_tip_x();
    let y = ui.get_tip_y();
    let w = ui.get_tip_w();
    let h = ui.get_tip_h();
    assert!(w > 8.0 && h > 8.0, "提示有尺寸 {w}x{h}");
    assert!(x >= -0.5, "提示左缘在窗口内 {x}");
    assert!(y >= -0.5, "提示上缘在窗口内 {y}");
    assert!(
        x + w <= ui.get_panel_width() + 0.5,
        "提示右缘在窗口内 {x}+{w} vs {}",
        ui.get_panel_width()
    );
    assert!(
        y + h <= ui.get_panel_height() + 0.5,
        "提示下缘在窗口内 {y}+{h} vs {}",
        ui.get_panel_height()
    );
}

fn wait_tip(ui: &Panel) {
    let delay = u64::try_from(ui.get_tip_delay().max(0)).unwrap_or(400);
    std::thread::sleep(std::time::Duration::from_millis(delay + 80));
    slint::platform::update_timers_and_animations();
}

#[test]
fn tabs_are_icon_hit_targets_and_names_appear_on_hover_or_focus() {
    let ui = open();
    assert!(ui.get_tab_hit_w() >= 40.0, "标签热区够宽");
    assert!(ui.get_tab_hit_h() >= 40.0, "标签热区够高");
    assert_eq!(ui.get_shown_tab_name().as_str(), "", "静止时不显示名称");
    let icons = icon_place(&ui);
    assert_icons_stay(&ui, icons);

    ui.invoke_focus_root();
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::Tab.into(),
    });
    slint::platform::update_timers_and_animations();
    assert_eq!(
        ui.get_shown_tab_name().as_str(),
        "收纳",
        "Tab 从面板焦点进入当前标签"
    );
    assert_icons_stay(&ui, icons);
    assert_tip_inside(&ui);
    let button_right = ui.get_tab_origin_x() + ui.get_tab_hit_w();
    assert!(
        ui.get_tip_x() + 0.5 >= button_right + ui.get_tip_gap(),
        "焦点提示在按钮右侧并留出间距 {} vs {button_right}+{}",
        ui.get_tip_x(),
        ui.get_tip_gap()
    );
    ui.invoke_focus_root();
    slint::platform::update_timers_and_animations();
    assert_eq!(
        ui.get_shown_tab_name().as_str(),
        "",
        "焦点回到内容后名称消失"
    );

    let selected = Rc::new(RefCell::new(Vec::new()));
    {
        let selected = Rc::clone(&selected);
        ui.on_tab_selected(move |index| selected.borrow_mut().push(index));
    }

    let (x, y) = center(&ui, 1);
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(x, y),
    });
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_tab_name().as_str(), "", "刚悬停时名称还不出现");
    assert_icons_stay(&ui, icons);
    wait_tip(&ui);
    assert_eq!(ui.get_shown_tab_name().as_str(), "便签");
    assert_icons_stay(&ui, icons);
    assert_tip_inside(&ui);
    assert!(
        ui.get_tip_x() + 0.5 >= button_right + ui.get_tip_gap(),
        "悬停提示在按钮右侧并留出间距"
    );
    assert!(
        ui.get_tip_w() >= 20.0,
        "名称按文字宽度画出，实际 {}",
        ui.get_tip_w()
    );

    let (next_x, next_y) = center(&ui, 2);
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(next_x, next_y),
    });
    slint::platform::update_timers_and_animations();
    assert_eq!(
        ui.get_shown_tab_name().as_str(),
        "收纳",
        "提示已经出现时换标签立刻跟着走"
    );
    assert_icons_stay(&ui, icons);

    ui.set_panel_width(320.0);
    slint::platform::update_timers_and_animations();
    assert_tip_inside(&ui);
    assert_icons_stay(&ui, icons);
    ui.set_panel_width(760.0);

    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(400.0, 200.0),
    });
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_tab_name().as_str(), "", "指针离开后名称消失");
    assert_icons_stay(&ui, icons);

    click(&ui, x, y);
    assert_eq!(selected.borrow().as_slice(), &[1]);

    // 指针还停在点过的标签上时，名称跟悬停走。键盘焦点单独测时先把指针移开。
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(400.0, 200.0),
    });
    slint::platform::update_timers_and_animations();
    ui.invoke_focus_tabs();
    slint::platform::update_timers_and_animations();
    assert_eq!(
        ui.get_shown_tab_name().as_str(),
        "收纳",
        "焦点先落在当前标签"
    );
    assert_icons_stay(&ui, icons);
    assert_tip_inside(&ui);

    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::DownArrow.into(),
    });
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_tab_name().as_str(), "应用");
    assert_icons_stay(&ui, icons);
    assert_tip_inside(&ui);
    assert!(selected.borrow().as_slice() == [1], "方向键不切换标签");

    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::Return.into(),
    });
    assert_eq!(selected.borrow().as_slice(), &[1, 3]);

    let escapes = Rc::new(RefCell::new(0));
    {
        let escapes = Rc::clone(&escapes);
        ui.on_escape(move || *escapes.borrow_mut() += 1);
    }
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::Escape.into(),
    });
    assert_eq!(*escapes.borrow(), 1, "没有待撤销状态时 Esc 收起");
}

#[test]
fn tab_tips_wait_out_hover_and_stay_inside_the_window() {
    let ui = open();
    let icons = icon_place(&ui);
    let (x, y) = center(&ui, 0);
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(x, y),
    });
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_tab_name().as_str(), "", "延迟结束前没有名称");
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(400.0, 300.0),
    });
    slint::platform::update_timers_and_animations();
    wait_tip(&ui);
    assert_eq!(
        ui.get_shown_tab_name().as_str(),
        "",
        "离开之后到点也不再出现"
    );
    assert_icons_stay(&ui, icons);

    ui.invoke_focus_tabs();
    slint::platform::update_timers_and_animations();
    for _ in 0..2 {
        ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: slint::platform::Key::UpArrow.into(),
        });
    }
    ui.set_panel_height(48.0);
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_tab_name().as_str(), "待办");
    assert_tip_inside(&ui);
    assert_icons_stay(&ui, icons);

    for _ in 0..4 {
        ui.window().dispatch_event(WindowEvent::KeyPressed {
            text: slint::platform::Key::DownArrow.into(),
        });
    }
    ui.set_panel_height(160.0);
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_tab_name().as_str(), "设置");
    assert_tip_inside(&ui);
    assert_icons_stay(&ui, icons);

    ui.set_panel_height(560.0);
    ui.set_panel_width(96.0);
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_tab_name().as_str(), "设置");
    assert_tip_inside(&ui);
    assert_icons_stay(&ui, icons);
    assert!(
        (ui.get_tab_icon_x() - icons.0).abs() < 0.5,
        "窄窗只挪提示，图标仍在 {}",
        ui.get_tab_icon_x()
    );
}

fn paint(path: &str) {
    const WIDTH: usize = 760;
    const HEIGHT: usize = 560;
    FRAME.with(|slot| {
        let window = slot.borrow().clone().expect("software window");
        window.request_redraw();
        let rendered = window.draw_if_needed(|renderer| {
            let mut buffer = vec![PremultipliedRgbaColor::default(); WIDTH * HEIGHT];
            renderer.render(&mut buffer, WIDTH);
            let mut file = std::fs::File::create(path).expect("ppm");
            use std::io::Write;
            write!(file, "P6\n{WIDTH} {HEIGHT}\n255\n").expect("header");
            for pixel in buffer {
                file.write_all(&[pixel.red, pixel.green, pixel.blue])
                    .expect("pixel");
            }
        });
        assert!(rendered, "这一帧要画出来");
    });
}

fn shelf_group(name: &str) -> ShelfGroupRow {
    ShelfGroupRow {
        name: name.into(),
        count: 3,
        todo: slint::SharedString::default(),
        checked: false,
    }
}

fn shelf_ref(name: &str) -> ShelfRefRow {
    ShelfRefRow {
        name: name.into(),
        icon: slint::Image::default(),
        has_icon: false,
        missing: false,
        web: false,
        folder: false,
        selected: false,
    }
}

/// 三张局部图的源画面。默认不跑，避免测试写文件。
/// `LANWORK_RAIL_SHOTS=/tmp/rail cargo test --bin lanwork rail_tooltip_shots -- --ignored --exact`
#[test]
#[ignore]
fn rail_tooltip_shots() {
    let dir = std::env::var("LANWORK_RAIL_SHOTS").unwrap_or_else(|_| "/tmp/rail-shots".into());
    std::fs::create_dir_all(&dir).expect("shot dir");
    let ui = open();
    ui.set_dark(false);
    ui.set_tab(2);
    let shelves = ui.global::<ShelvesPageData>();
    shelves.set_groups(slint::ModelRc::new(slint::VecModel::from(vec![
        shelf_group("资料"),
        shelf_group("网页"),
        shelf_group("进行中"),
    ])));
    shelves.set_selected_group(0);
    shelves.set_group_name("资料".into());
    shelves.set_refs(slint::ModelRc::new(slint::VecModel::from(vec![
        shelf_ref("说明.txt"),
        shelf_ref("记录.pdf"),
        shelf_ref("封面.png"),
    ])));
    slint::platform::update_timers_and_animations();

    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(420.0, 280.0),
    });
    ui.invoke_focus_root();
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_tab_name().as_str(), "");
    paint(&format!("{dir}/idle.ppm"));

    let (x, y) = center(&ui, 2);
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(x, y),
    });
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_tab_name().as_str(), "");
    wait_tip(&ui);
    assert_eq!(ui.get_shown_tab_name().as_str(), "收纳");
    paint(&format!("{dir}/hover.ppm"));

    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(420.0, 280.0),
    });
    slint::platform::update_timers_and_animations();
    ui.invoke_focus_tabs();
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_tab_name().as_str(), "收纳");
    paint(&format!("{dir}/focus.ppm"));
}

fn press_escape(ui: &Panel) {
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::Escape.into(),
    });
}

fn press_escape_repeat(ui: &Panel) {
    ui.window().dispatch_event(WindowEvent::KeyPressRepeated {
        text: slint::platform::Key::Escape.into(),
    });
}

#[test]
fn rail_escape_follows_the_panel_dismiss_order() {
    let ui = open();
    let escapes = Rc::new(Cell::new(0));
    {
        let escapes = Rc::clone(&escapes);
        ui.on_escape(move || escapes.set(escapes.get() + 1));
    }

    ui.set_tab(1);
    let notes = ui.global::<NotesPageData>();
    notes.set_selecting(true);
    let note_selecting = Rc::new(Cell::new(true));
    {
        let note_selecting = Rc::clone(&note_selecting);
        notes.on_set_selecting(move |active| note_selecting.set(active));
    }
    ui.invoke_focus_tabs();
    press_escape(&ui);
    assert!(!note_selecting.get(), "便签多选先退出");
    assert_eq!(escapes.get(), 0, "便签多选时侧栏 Esc 不收起");
    notes.set_selecting(false);

    ui.set_tab(2);
    ui.invoke_focus_tabs();
    let shelves = ui.global::<ShelvesPageData>();
    let steps = Rc::new(RefCell::new(Vec::new()));
    {
        let steps = Rc::clone(&steps);
        shelves.on_close_menu(move || {
            steps.borrow_mut().push("menu");
        });
    }
    {
        let steps = Rc::clone(&steps);
        shelves.on_cancel_remove_refs(move || {
            steps.borrow_mut().push("refs-confirm");
        });
    }
    {
        let steps = Rc::clone(&steps);
        shelves.on_cancel_delete_groups(move || {
            steps.borrow_mut().push("groups-confirm");
        });
    }
    {
        let steps = Rc::clone(&steps);
        shelves.on_delete_group(move || {
            steps.borrow_mut().push("deleted");
        });
    }
    {
        let steps = Rc::clone(&steps);
        shelves.on_clear_refs(move || {
            steps.borrow_mut().push("refs");
        });
    }
    {
        let steps = Rc::clone(&steps);
        shelves.on_set_group_selecting(move |active| {
            if !active {
                steps.borrow_mut().push("groups");
            }
        });
    }
    shelves.set_group_menu_open(true);
    shelves.set_renaming(true);
    shelves.set_menu_open(true);
    shelves.set_confirm_refs(true);
    shelves.set_confirm_delete(true);
    shelves.set_confirm_groups(true);
    shelves.set_ref_count(1);
    shelves.set_selecting(true);
    ui.invoke_focus_tabs();

    press_escape(&ui);
    assert!(!shelves.get_group_menu_open(), "先关分组⋯");
    assert!(shelves.get_menu_open());
    assert!(shelves.get_renaming());
    assert!(steps.borrow().is_empty());
    assert_eq!(escapes.get(), 0);

    press_escape(&ui);
    shelves.set_menu_open(false);
    assert!(shelves.get_renaming(), "右键菜单先于重命名");
    assert_eq!(steps.borrow().as_slice(), ["menu"]);
    assert_eq!(escapes.get(), 0);

    press_escape(&ui);
    assert!(!shelves.get_renaming(), "再取消重命名");
    assert_eq!(steps.borrow().as_slice(), ["menu"]);
    assert_eq!(escapes.get(), 0);

    press_escape(&ui);
    shelves.set_confirm_refs(false);
    assert_eq!(steps.borrow().as_slice(), ["menu", "refs-confirm"]);
    assert!(shelves.get_confirm_delete(), "条目确认先于分组删除确认");
    assert_eq!(escapes.get(), 0);

    press_escape(&ui);
    assert!(!shelves.get_confirm_delete(), "单组确认删除先撤回，不删除");
    assert!(shelves.get_confirm_groups());
    assert_eq!(steps.borrow().as_slice(), ["menu", "refs-confirm"]);
    assert_eq!(escapes.get(), 0);

    press_escape(&ui);
    shelves.set_confirm_groups(false);
    assert_eq!(
        steps.borrow().as_slice(),
        ["menu", "refs-confirm", "groups-confirm"]
    );
    assert_eq!(escapes.get(), 0);

    press_escape(&ui);
    shelves.set_ref_count(0);
    assert_eq!(
        steps.borrow().as_slice(),
        ["menu", "refs-confirm", "groups-confirm", "refs"]
    );
    assert_eq!(escapes.get(), 0);

    press_escape(&ui);
    shelves.set_selecting(false);
    assert_eq!(
        steps.borrow().as_slice(),
        ["menu", "refs-confirm", "groups-confirm", "refs", "groups"]
    );
    assert!(!steps.borrow().contains(&"deleted"), "Esc 不触发删除分组");
    assert_eq!(escapes.get(), 0, "分组多选退出后仍不收起");

    press_escape(&ui);
    assert_eq!(escapes.get(), 1, "各层都清掉之后 Esc 才收起");

    ui.set_tab(0);
    ui.set_triage_open(true);
    let keys = Rc::new(RefCell::new(Vec::new()));
    {
        let keys = Rc::clone(&keys);
        let weak = ui.as_weak();
        ui.on_triage_key(move |code, _modified| {
            keys.borrow_mut().push(code);
            if code == KEY_ESCAPE
                && let Some(ui) = weak.upgrade()
            {
                ui.set_triage_open(false);
            }
            true
        });
    }
    ui.invoke_focus_tabs();
    let escapes_before = escapes.get();
    press_escape(&ui);
    assert_eq!(keys.borrow().as_slice(), [KEY_ESCAPE]);
    assert!(!ui.get_triage_open(), "侧栏上的一次 Esc 退出处理模式");
    assert_eq!(escapes.get(), escapes_before);
    press_escape_repeat(&ui);
    press_escape_repeat(&ui);
    assert_eq!(escapes.get(), escapes_before, "按住 Esc 的重复不收起");
    ui.window().dispatch_event(WindowEvent::KeyReleased {
        text: slint::platform::Key::Escape.into(),
    });
    press_escape(&ui);
    assert_eq!(escapes.get(), escapes_before + 1, "松开后再按才收起");

    let source = include_str!("../ui/panel.slint");
    let escape_fn = source
        .split("function panel-escape")
        .nth(1)
        .expect("panel-escape")
        .split("function triage-code")
        .next()
        .expect("panel-escape body");
    let preedit = escape_fn.find("preedit-text").expect("组字检查");
    let call = escape_fn.find("triage-handle").expect("triage-handle");
    let hide = escape_fn.find("root.escape()").expect("收起");
    assert!(preedit < call && call < hide, "组字中的 Esc 先交还输入法");
    let rail = source.split("rail := FocusScope").nth(1).expect("rail");
    let rail_keys = rail.split("key-pressed").nth(1).expect("rail keys");
    let rail_keys = rail_keys.split("TouchArea").next().unwrap_or(rail_keys);
    assert!(!rail_keys.contains("root.escape()"), "侧栏不再直接收起");
}

#[test]
fn single_group_delete_confirm_cancels_without_deleting_or_hiding() {
    let ui = open();
    let escapes = Rc::new(Cell::new(0));
    {
        let escapes = Rc::clone(&escapes);
        ui.on_escape(move || escapes.set(escapes.get() + 1));
    }
    let shelves = ui.global::<ShelvesPageData>();
    let deleted = Rc::new(Cell::new(0));
    {
        let deleted = Rc::clone(&deleted);
        shelves.on_delete_group(move || deleted.set(deleted.get() + 1));
    }
    shelves.set_confirm_delete(true);
    ui.invoke_focus_tabs();

    press_escape(&ui);
    assert!(!shelves.get_confirm_delete(), "第一次只关掉确认");
    assert_eq!(deleted.get(), 0, "不触发删除");
    assert_eq!(escapes.get(), 0, "面板还在");

    press_escape(&ui);
    assert_eq!(escapes.get(), 1, "确认关掉之后才收起");
    assert_eq!(deleted.get(), 0);
}

#[test]
fn composing_escape_on_the_rail_does_not_cancel_rename_or_hide() {
    let ui = open();
    let escapes = Rc::new(Cell::new(0));
    {
        let escapes = Rc::clone(&escapes);
        ui.on_escape(move || escapes.set(escapes.get() + 1));
    }
    let shelves = ui.global::<ShelvesPageData>();
    let closed = Rc::new(Cell::new(0));
    {
        let closed = Rc::clone(&closed);
        shelves.on_close_menu(move || closed.set(closed.get() + 1));
    }
    shelves.set_menu_open(true);
    shelves.set_renaming(true);
    shelves.set_composing(true);
    ui.invoke_focus_tabs();

    press_escape(&ui);
    assert!(shelves.get_menu_open(), "组字时不关右键菜单");
    assert!(shelves.get_renaming(), "组字时不取消重命名");
    assert_eq!(closed.get(), 0);
    assert_eq!(escapes.get(), 0, "组字时不收起");

    shelves.set_composing(false);
    ui.invoke_focus_tabs();
    press_escape(&ui);
    assert_eq!(closed.get(), 1, "组字结束后先关菜单");
    assert!(shelves.get_renaming());
    assert_eq!(escapes.get(), 0);
}
