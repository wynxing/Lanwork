//! 应用页的布局和点击检查。用软件渲染把「应用」标签摆出来。
//! 坐标由页面写进 `AppsPageData`，不依赖某一种中文字宽。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::platform::software_renderer::RepaintBufferType;
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{AppToolCard, AppsPageData, Panel};

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

thread_local! {
    static FRAME: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
}

fn open() -> Panel {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(760, 560));
    if slint::platform::set_platform(Box::new(Headless(Rc::clone(&window)))).is_ok() {
        FRAME.with(|slot| *slot.borrow_mut() = Some(window));
    }
    let ui = Panel::new().expect("panel");
    ui.set_panel_width(760.0);
    ui.set_panel_height(560.0);
    ui.set_tab(3);
    ui.window().set_size(slint::PhysicalSize::new(760, 560));
    ui.show().expect("show");
    slint::platform::update_timers_and_animations();
    ui
}

fn draw_frame() {
    FRAME.with(|slot| {
        let Some(window) = slot.borrow().clone() else {
            return;
        };
        window.request_redraw();
        let _ = window.draw_if_needed(|renderer| {
            use slint::platform::software_renderer::Rgb565Pixel;
            let mut buffer = vec![Rgb565Pixel(0); 760 * 560];
            renderer.render(&mut buffer, 760);
        });
    });
}

fn settle(ui: &Panel) {
    // 先画出列表，卡片才存在。再改 layout-tick，卡片的 changed 才会把坐标写出来。
    slint::platform::update_timers_and_animations();
    draw_frame();
    let data = ui.global::<AppsPageData>();
    data.set_layout_tick(data.get_layout_tick() + 1);
    slint::platform::update_timers_and_animations();
    draw_frame();
    data.set_layout_tick(data.get_layout_tick() + 1);
    slint::platform::update_timers_and_animations();
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
    settle(ui);
}

fn card(id: &str, name: &str, detail: &str, mark: &str, enabled: bool) -> AppToolCard {
    AppToolCard {
        tool_id: id.into(),
        name: name.into(),
        detail: detail.into(),
        mark: mark.into(),
        has_icon: false,
        icon: slint::Image::default(),
        enabled,
        builtin: true,
    }
}

fn set_tools(ui: &Panel, cards: Vec<AppToolCard>) {
    ui.global::<AppsPageData>()
        .set_tools(ModelRc::new(VecModel::from(cards)));
    settle(ui);
}

#[test]
fn empty_builtin_has_no_import_and_custom_keeps_the_import_card() {
    let ui = open();
    let data = ui.global::<AppsPageData>();
    data.set_category(0);
    data.set_show_import(false);
    set_tools(&ui, Vec::new());
    assert!(data.get_empty_shown());
    assert_eq!(data.get_empty_text().as_str(), "没有工具");
    assert!(!data.get_import_shown(), "内置没有导入+");
    assert!(data.get_builtin_w() > 0.0);
    assert!(data.get_custom_x() > data.get_builtin_x());
    assert_eq!(data.get_builtin_h(), 40.0);

    let categories = Rc::new(RefCell::new(Vec::new()));
    {
        let categories = Rc::clone(&categories);
        data.on_select_category(move |index| categories.borrow_mut().push(index));
    }
    click(
        &ui,
        data.get_custom_x() + data.get_custom_w() / 2.0,
        data.get_custom_y() + data.get_custom_h() / 2.0,
    );
    assert_eq!(categories.borrow().as_slice(), [1]);
    data.set_category(1);
    data.set_show_import(true);
    settle(&ui);
    assert!(data.get_empty_shown());
    assert_eq!(data.get_empty_text().as_str(), "没有工具");
    assert!(data.get_import_shown(), "自定义没有工具时导入+仍在");
    assert!(data.get_import_h() > 0.0);
    assert!(data.get_import_y() > data.get_empty_y());
}

#[test]
fn a_card_shows_name_detail_mark_and_the_float_button_follows_the_switch() {
    let ui = open();
    let data = ui.global::<AppsPageData>();
    data.set_category(0);
    data.set_show_import(false);
    set_tools(
        &ui,
        vec![card(
            "example.quota",
            "API 额度",
            "服务商接口的剩余额度",
            "A",
            true,
        )],
    );
    assert!(!data.get_empty_shown());
    assert!(!data.get_import_shown());
    assert_eq!(data.get_card_name().as_str(), "API 额度");
    assert_eq!(data.get_card_detail().as_str(), "服务商接口的剩余额度");
    assert_eq!(data.get_card_mark().as_str(), "A");
    assert!(data.get_mark_shown());
    assert!(!data.get_icon_shown());
    assert_eq!(data.get_card_h(), 72.0);
    assert!(data.get_open_enabled());
    assert!(data.get_switch_checked());
    assert!(data.get_open_x() > data.get_card_x());
    assert!(data.get_switch_x() > data.get_open_x());
    assert!(data.get_more_x() > data.get_switch_x());

    let opened = Rc::new(RefCell::new(Vec::new()));
    let toggles = Rc::new(RefCell::new(Vec::new()));
    {
        let opened = Rc::clone(&opened);
        data.on_open_float(move |id| opened.borrow_mut().push(id.to_string()));
        let toggles = Rc::clone(&toggles);
        data.on_toggle_enabled(move |id, enabled| {
            toggles.borrow_mut().push((id.to_string(), enabled));
        });
    }
    click(
        &ui,
        data.get_open_x() + data.get_open_w() / 2.0,
        data.get_open_y() + data.get_open_h() / 2.0,
    );
    assert_eq!(opened.borrow().as_slice(), ["example.quota"]);
    click(
        &ui,
        data.get_switch_x() + data.get_switch_w() / 2.0,
        data.get_switch_y() + data.get_switch_h() / 2.0,
    );
    assert_eq!(
        toggles.borrow().as_slice(),
        [("example.quota".to_owned(), false)]
    );
    assert!(data.get_switch_checked(), "开关不自己翻转");

    set_tools(
        &ui,
        vec![card(
            "example.quota",
            "API 额度",
            "服务商接口的剩余额度",
            "A",
            false,
        )],
    );
    assert!(!data.get_open_enabled());
    let before = opened.borrow().len();
    click(
        &ui,
        data.get_open_x() + data.get_open_w() / 2.0,
        data.get_open_y() + data.get_open_h() / 2.0,
    );
    assert_eq!(opened.borrow().len(), before, "关闭时悬浮窗点不了");
}

#[test]
fn the_more_menu_lists_the_items_it_is_given_and_import_is_a_separate_card() {
    let ui = open();
    let data = ui.global::<AppsPageData>();
    data.set_show_import(true);
    set_tools(
        &ui,
        vec![card("example.quota", "API 额度", "剩余额度", "额", true)],
    );
    let menus = Rc::new(Cell::new(0));
    let chosen = Rc::new(RefCell::new(Vec::new()));
    let closed = Rc::new(Cell::new(0));
    let imports = Rc::new(Cell::new(0));
    {
        let menus = Rc::clone(&menus);
        data.on_open_menu(move |index| {
            menus.set(menus.get() + 1);
            assert_eq!(index, 0);
        });
        let chosen = Rc::clone(&chosen);
        data.on_menu_chosen(move |index| chosen.borrow_mut().push(index));
        let closed = Rc::clone(&closed);
        data.on_close_menu(move || closed.set(closed.get() + 1));
        let imports = Rc::clone(&imports);
        data.on_import_clicked(move || imports.set(imports.get() + 1));
    }
    click(
        &ui,
        data.get_more_x() + data.get_more_w() / 2.0,
        data.get_more_y() + data.get_more_h() / 2.0,
    );
    assert_eq!(menus.get(), 1);
    data.set_menu_index(0);
    data.set_menu_items(ModelRc::new(VecModel::from(vec![
        SharedString::from("更新"),
        SharedString::from("卸载"),
        SharedString::from("查看权限"),
    ])));
    data.set_menu_open(true);
    settle(&ui);
    assert!(data.get_menu_shown());
    assert_eq!(data.get_menu_first().as_str(), "更新");
    assert!(data.get_menu_w() >= 160.0);
    click(
        &ui,
        data.get_menu_x() + 24.0,
        data.get_menu_y() + 4.0 + 16.0,
    );
    assert_eq!(chosen.borrow().as_slice(), [0]);

    data.set_menu_open(true);
    settle(&ui);
    click(&ui, 24.0, data.get_empty_y());
    assert!(closed.get() >= 1, "点菜单外关掉菜单");
    data.set_menu_open(false);
    settle(&ui);

    assert!(data.get_import_shown());
    click(
        &ui,
        data.get_import_x() + 40.0,
        data.get_import_y() + data.get_import_h() / 2.0,
    );
    assert_eq!(imports.get(), 1);
}

#[test]
fn the_import_card_lists_permissions_and_the_update_note_is_kept() {
    let ui = open();
    let data = ui.global::<AppsPageData>();
    data.set_dialog_kind(SharedString::from("perm"));
    data.set_dialog_update(true);
    data.set_dialog_confirm(true);
    data.set_dialog_name(SharedString::from("API 额度"));
    data.set_dialog_detail(SharedString::from("服务商接口的剩余额度"));
    data.set_dialog_lines(ModelRc::new(VecModel::from(vec![
        SharedString::from("访问 api.example.com"),
        SharedString::from("使用你填写的 API 密钥"),
    ])));
    data.set_dialog_note(SharedString::from("保留数据"));
    data.set_dialog_open(true);
    settle(&ui);
    assert!(data.get_dialog_shown());
    assert_eq!(data.get_dialog_kicker().as_str(), "更新");
    assert_eq!(data.get_dialog_note_text().as_str(), "保留数据");
    assert!(data.get_confirm_shown());
    assert_eq!(data.get_confirm_label().as_str(), "确认");
    assert_eq!(data.get_dialog_line().as_str(), "访问 api.example.com");
    assert!(!data.get_danger_shown());

    data.set_dialog_kind(SharedString::from("uninstall"));
    data.set_dialog_confirm(false);
    data.set_dialog_update(false);
    data.set_dialog_note(SharedString::from("会删除这个工具的存储和密钥。"));
    settle(&ui);
    assert!(data.get_danger_shown());
    assert!(!data.get_confirm_shown());
}
