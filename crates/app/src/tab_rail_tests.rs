//! 面板左侧标签。图标可点，名称只在悬停或键盘焦点上出现。

use std::cell::RefCell;
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::Panel;

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
    assert_eq!(*escapes.borrow(), 1, "焦点在标签上时 Esc 仍收起");
}
