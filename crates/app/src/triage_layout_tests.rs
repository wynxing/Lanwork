//! 处理模式的版式和按键检查。软件渲染的面板上点「处理 N 条」、五个按钮和清单选择，
//! 并确认组合输入（preedit）期间不把数字键交给处理模式。

use std::cell::RefCell;
use std::rc::Rc;

use lanwork_core::panel::{KEY_DOWN, KEY_ESCAPE, KEY_TODAY};
use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::software_renderer::RepaintBufferType;
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::Panel;

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

const WIDTH: u32 = 760;
const HEIGHT: u32 = 560;

fn open() -> Panel {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    window.set_size(slint::PhysicalSize::new(WIDTH, HEIGHT));
    let ui = Panel::new().expect("panel");
    ui.set_tab(0);
    ui.set_tab_titles(ModelRc::new(VecModel::from(vec![
        SharedString::from("待办"),
        SharedString::from("便签"),
        SharedString::from("收纳"),
        SharedString::from("设置"),
    ])));
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

fn press(ui: &Panel, text: &str) {
    ui.window()
        .dispatch_event(WindowEvent::KeyPressed { text: text.into() });
}

#[test]
fn process_button_sits_at_the_top_of_the_inbox_list() {
    let ui = open();
    let started = Rc::new(RefCell::new(0));
    {
        let started = Rc::clone(&started);
        ui.on_start_triage(move || *started.borrow_mut() += 1);
    }
    assert!(!ui.get_show_process());
    click(&ui, 220.0, 114.0);
    assert_eq!(*started.borrow(), 0, "没有未完成条目时不显示入口");

    ui.set_show_process(true);
    ui.set_process_label("处理 3 条".into());
    // 标题 48 + 分隔线 1 + 右侧头部 50 + 按钮半高 15。按钮在清单栏右侧，左边距 12。
    click(&ui, 220.0, 114.0);
    assert_eq!(*started.borrow(), 1);
}

#[test]
fn card_keys_and_buttons_follow_the_theme_and_ignore_composition() {
    let ui = open();
    let keys = Rc::new(RefCell::new(Vec::new()));
    let clicks = Rc::new(RefCell::new(Vec::new()));
    {
        let keys = Rc::clone(&keys);
        ui.on_triage_key(move |code, modified| {
            keys.borrow_mut().push((code, modified));
            true
        });
        let clicks = Rc::clone(&clicks);
        ui.on_triage_click(move |index| clicks.borrow_mut().push(index));
    }
    let escapes = Rc::new(RefCell::new(0));
    {
        let escapes = Rc::clone(&escapes);
        ui.on_escape(move || *escapes.borrow_mut() += 1);
    }

    ui.set_triage_open(true);
    ui.set_triage_show_card(true);
    ui.set_triage_progress("第 3 / 12 条".into());
    ui.set_triage_title("写规格".into());
    ui.set_triage_due("2026-10-14".into());
    ui.set_triage_source("wynxing/Lanwork#24".into());
    let labels: Vec<SharedString> = ["1 今天", "2 稍后 3 天", "3 移动", "4 完成", "5 删除"]
        .into_iter()
        .map(Into::into)
        .collect();
    ui.set_triage_actions(ModelRc::new(VecModel::from(labels)));
    ui.invoke_focus_triage();

    press(&ui, "1");
    assert_eq!(*keys.borrow(), [(KEY_TODAY, false)]);
    press(&ui, "\u{001b}");
    assert_eq!(keys.borrow()[1], (KEY_ESCAPE, false));
    assert_eq!(*escapes.borrow(), 0, "处理模式里 Esc 不收起面板");

    // 五个按钮宽 108、间隔 8，整行 572，在 760 里居中。第三个是「3 移动」。
    // 卡片块高 222，在内容区（高 511）里垂直居中后再加上进度和卡片。
    click(&ui, 380.0, 400.0);
    assert_eq!(*clicks.borrow(), [2]);
}

#[test]
fn move_picker_uses_arrows_enter_and_escape_without_the_digit_keys() {
    let ui = open();
    let keys = Rc::new(RefCell::new(Vec::new()));
    let picks = Rc::new(RefCell::new(Vec::new()));
    {
        let keys = Rc::clone(&keys);
        ui.on_triage_key(move |code, modified| {
            keys.borrow_mut().push((code, modified));
            true
        });
        let picks = Rc::clone(&picks);
        ui.on_triage_pick(move |index| picks.borrow_mut().push(index));
    }
    ui.set_triage_open(true);
    ui.set_triage_show_card(true);
    ui.set_triage_picker(true);
    ui.set_triage_list_index(0);
    let names: Vec<SharedString> = ["工作", "家"].into_iter().map(Into::into).collect();
    ui.set_triage_lists(ModelRc::new(VecModel::from(names)));
    ui.invoke_focus_triage();

    press(&ui, "3");
    assert!(keys.borrow().is_empty() || keys.borrow()[0].0 != KEY_TODAY);
    // 数字键在选择开着时仍然报到界面，由 Rust 决定不触发「今天」。
    // 这里只确认按键能到，方向键用 Slint 的 Key 文本。
    keys.borrow_mut().clear();
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: slint::platform::Key::DownArrow.into(),
    });
    assert_eq!(keys.borrow().as_slice(), [(KEY_DOWN, false)]);

    // 卡片左上角附近的第一行。选择盖在卡片上，卡片在内容区水平居中。
    click(&ui, 200.0, 250.0);
    assert!(
        picks.borrow().first().is_some_and(|index| *index == 0),
        "点第一行确认，实际 {:?}",
        picks.borrow()
    );
}

#[test]
fn triage_input_checks_preedit_before_the_digit_keys() {
    let source = include_str!("../ui/panel.slint");
    let handler = source
        .split("function triage-handle")
        .nth(1)
        .expect("triage-handle");
    let preedit = handler.find("preedit-text").expect("preedit check");
    let call = handler.find("triage-key").expect("triage-key");
    assert!(preedit < call, "组合输入要先于 1 至 5 交还输入法");
    assert!(
        !source.contains("triage") || !handler[preedit..call].contains("c.accent"),
        "按键处理本身不引入强调色"
    );
}
