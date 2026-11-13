//! 面板左侧标签。图标可点，名称只在悬停或键盘焦点上出现。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use lanwork_core::panel::KEY_ESCAPE;

use crate::{NotesPageData, Panel, ShelvesPageData};

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

const TITLES: [&str; 5] = ["待办", "便签", "收纳", "应用", "设置"];

fn open() -> Panel {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    window.set_size(slint::PhysicalSize::new(760, 560));
    let ui = Panel::new().expect("panel");
    ui.set_tab(2);
    ui.set_tab_titles(ModelRc::new(VecModel::from(
        TITLES
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
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

#[test]
fn tabs_are_icon_hit_targets_and_names_appear_on_hover_or_focus() {
    let ui = open();
    assert!(ui.get_tab_hit_w() >= 40.0, "标签热区够宽");
    assert!(ui.get_tab_hit_h() >= 40.0, "标签热区够高");
    assert_eq!(ui.get_shown_tab_name().as_str(), "", "静止时不显示名称");

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
    assert_eq!(ui.get_shown_tab_name().as_str(), "便签");
    let tip_x = ui.get_tip_x();
    let tip_w = ui.get_tip_w();
    let icon_x = ui.get_tip_icon_x();
    assert!(tip_w >= 20.0, "名称按文字宽度画出，实际 {tip_w}");
    assert!(tip_x >= 0.0, "名称留在窗口内，实际 {tip_x}");
    assert!(
        tip_x + tip_w <= icon_x + 0.5,
        "名称在图标左侧 {tip_x}+{tip_w} vs {icon_x}"
    );
    assert!(tip_x + tip_w <= ui.get_shelf_zone_x(), "名称不进入内容区");
    ui.set_panel_width(320.0);
    slint::platform::update_timers_and_animations();
    assert!(
        ui.get_tip_x() >= 0.0 && ui.get_tip_x() + ui.get_tip_w() <= ui.get_shelf_zone_x(),
        "窄面板上名称仍完整留在侧栏里"
    );
    ui.set_panel_width(760.0);

    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(400.0, 200.0),
    });
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_tab_name().as_str(), "", "指针离开后名称消失");

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

    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::DownArrow.into(),
    });
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_tab_name().as_str(), "应用");
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
    shelves.set_confirm_groups(true);
    shelves.set_ref_count(1);
    shelves.set_selecting(true);

    press_escape(&ui);
    assert!(!shelves.get_group_menu_open(), "先关分组⋯");
    assert!(shelves.get_renaming());
    assert!(steps.borrow().is_empty());
    assert_eq!(escapes.get(), 0);

    press_escape(&ui);
    assert!(!shelves.get_renaming(), "再取消重命名");
    assert!(shelves.get_menu_open());
    assert_eq!(escapes.get(), 0);

    press_escape(&ui);
    shelves.set_menu_open(false);
    assert_eq!(steps.borrow().as_slice(), ["menu"]);
    assert_eq!(escapes.get(), 0);

    press_escape(&ui);
    shelves.set_confirm_refs(false);
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
