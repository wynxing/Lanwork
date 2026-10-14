//! 收纳页多选的布局和点击。软件渲染摆出面板的收纳标签，确认「选择」、勾选和底部操作条互不重叠，
//! 分组行和条目行在行外松开不算点击，右键菜单点得到，Esc 清选择或退出分组多选时不收起面板。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::{Panel, ShelfGroupRow, ShelfMenuRow, ShelfRefRow, ShelvesPageData};

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn group(name: &str, checked: bool) -> ShelfGroupRow {
    ShelfGroupRow {
        name: name.into(),
        count: 2,
        todo: SharedString::default(),
        checked,
    }
}

fn item(name: &str, selected: bool) -> ShelfRefRow {
    ShelfRefRow {
        name: name.into(),
        icon: slint::Image::default(),
        has_icon: false,
        missing: false,
        selected,
    }
}

fn menu_row(label: &str, enabled: bool) -> ShelfMenuRow {
    ShelfMenuRow {
        label: label.into(),
        enabled,
        danger: false,
    }
}

fn open() -> Panel {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    window.set_size(slint::PhysicalSize::new(760, 560));
    let ui = Panel::new().expect("panel");
    ui.set_dark(false);
    ui.set_tab(2);
    let data = ui.global::<ShelvesPageData>();
    data.set_groups(ModelRc::new(VecModel::from(vec![
        group("资料", false),
        group("网页", false),
    ])));
    data.set_selected_group(0);
    data.set_group_name("资料".into());
    data.set_refs(ModelRc::new(VecModel::from(vec![
        item("说明.txt", false),
        item("记录.txt", true),
    ])));
    data.set_selected_ref(-1);
    data.set_selecting(false);
    data.set_group_selected_count(0);
    data.set_confirm_groups(false);
    data.set_ref_count(0);
    data.set_confirm_refs(false);
    data.set_menu_open(false);
    data.set_ref_dragging(false);
    ui.show().expect("show");
    slint::platform::update_timers_and_animations();
    refresh(&ui.global::<ShelvesPageData>());
    ui
}

fn refresh(data: &ShelvesPageData) {
    data.set_geometry_refresh(data.get_geometry_refresh().wrapping_add(1));
    slint::platform::update_timers_and_animations();
}

fn click(ui: &Panel, x: f32, y: f32) {
    stroke(ui, (x, y), (x, y), PointerEventButton::Left);
}

fn stroke(ui: &Panel, from: (f32, f32), to: (f32, f32), button: PointerEventButton) {
    let from = slint::LogicalPosition::new(from.0, from.1);
    let to = slint::LogicalPosition::new(to.0, to.1);
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: from });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: from,
        button,
    });
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: to });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: to,
        button,
    });
}

fn press_escape(ui: &Panel) {
    ui.invoke_focus_root();
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: "\u{001b}".into(),
    });
}

/// 分组行高与 `shelvespage.slint` 里没有待办标题的行一致。
const GROUP_ROW: f32 = 40.0;
/// 条目行高与 `RefsList` 的 `row-height` 一致。
const REF_ROW: f32 = 40.0;

#[test]
fn group_select_bar_sits_between_the_list_and_new_group() {
    let ui = open();
    let data = ui.global::<ShelvesPageData>();
    let selecting = Rc::new(RefCell::new(Vec::new()));
    let escapes = Rc::new(Cell::new(0));
    {
        let selecting = Rc::clone(&selecting);
        data.on_set_group_selecting(move |active| selecting.borrow_mut().push(active));
        let escapes = Rc::clone(&escapes);
        ui.on_escape(move || escapes.set(escapes.get() + 1));
    }

    let choose_y = data.get_choose_y();
    let choose_h = data.get_choose_h();
    let list_y = data.get_list_y();
    let list_h = data.get_list_h();
    let new_y = data.get_new_y();
    assert!(choose_h > 0.0, "选择按钮有高度");
    assert!(choose_y > 40.0, "几何是窗口坐标，含标题栏");
    assert!(list_h > 0.0, "分组列表有高度");
    assert!(choose_y + choose_h <= list_y, "选择在列表上方");
    assert_eq!(data.get_bar_h(), 0.0, "未多选时操作条不占高度");
    assert!(
        list_y + list_h <= new_y + 1.0,
        "未多选时列表不压住新建 {list_y}+{list_h} vs {new_y}"
    );
    assert_eq!(data.get_count_label().as_str(), "已选 0 个");

    click(
        &ui,
        data.get_choose_x() + data.get_choose_w() / 2.0,
        choose_y + choose_h / 2.0,
    );
    assert_eq!(*selecting.borrow(), [true]);

    data.set_selecting(true);
    data.set_group_selected_count(1);
    refresh(&data);

    let list_y = data.get_list_y();
    let list_h = data.get_list_h();
    let bar_y = data.get_bar_y();
    let bar_h = data.get_bar_h();
    let new_y = data.get_new_y();
    assert!(bar_h > 40.0, "操作条有计数和按钮");
    assert!(
        list_y + list_h <= bar_y + 1.0,
        "列表不压住操作条 {list_y}+{list_h} vs {bar_y}"
    );
    assert!(
        bar_y + bar_h <= new_y + 1.0,
        "操作条不压住新建 {bar_y}+{bar_h} vs {new_y}"
    );
    assert_eq!(data.get_count_label().as_str(), "已选 1 个");
    assert!(data.get_delete_h() > 0.0, "删除按钮可点");

    press_escape(&ui);
    assert_eq!(selecting.borrow().as_slice(), &[true, false]);
    assert_eq!(escapes.get(), 0, "分组多选时 Esc 不收起面板");

    data.set_selecting(false);
    refresh(&data);
    press_escape(&ui);
    assert_eq!(escapes.get(), 1, "退出多选之后 Esc 仍收起面板");
}

#[test]
fn releasing_outside_a_group_or_item_row_does_not_activate_it() {
    let ui = open();
    let data = ui.global::<ShelvesPageData>();
    let groups = Rc::new(RefCell::new(Vec::new()));
    let refs = Rc::new(RefCell::new(Vec::new()));
    {
        let groups = Rc::clone(&groups);
        data.on_group_clicked(move |index, ctrl, shift| {
            groups.borrow_mut().push((index, ctrl, shift));
        });
        let refs = Rc::clone(&refs);
        data.on_ref_click(move |index, ctrl, shift| {
            refs.borrow_mut().push((index, ctrl, shift));
        });
    }

    let gx = data.get_groups_x() + 16.0;
    let gy = data.get_groups_y();
    assert!(data.get_groups_h() > GROUP_ROW, "分组列表够两行");
    let row0 = gy + GROUP_ROW / 2.0;
    let row1 = gy + GROUP_ROW + GROUP_ROW / 2.0;
    stroke(&ui, (gx, row0), (gx, row1), PointerEventButton::Left);
    assert!(groups.borrow().is_empty(), "松在下一行没有动作");
    stroke(
        &ui,
        (gx, row0),
        (gx + 400.0, row0),
        PointerEventButton::Left,
    );
    assert!(groups.borrow().is_empty(), "松在列表右侧没有动作");
    click(&ui, gx, row0);
    assert_eq!(groups.borrow().as_slice(), &[(0, false, false)]);

    data.set_selecting(true);
    refresh(&data);
    let before = groups.borrow().len();
    stroke(&ui, (gx, row0), (gx, row1), PointerEventButton::Left);
    assert_eq!(groups.borrow().len(), before, "多选时松在行外也不改勾选");

    let rx = data.get_refs_x() + 20.0;
    let ry = data.get_refs_y();
    assert!(data.get_refs_h() > REF_ROW, "条目列表够两行");
    let item0 = ry + REF_ROW / 2.0;
    let item1 = ry + REF_ROW + REF_ROW / 2.0;
    stroke(&ui, (rx, item0), (rx, item1), PointerEventButton::Left);
    assert!(refs.borrow().is_empty(), "条目松在下一行没有动作");
    click(&ui, rx, item0);
    assert_eq!(refs.borrow().as_slice(), &[(0, false, false)]);
}

#[test]
fn escape_clears_item_selection_before_leaving_group_select() {
    let ui = open();
    let data = ui.global::<ShelvesPageData>();
    let cleared = Rc::new(Cell::new(0));
    let selecting = Rc::new(RefCell::new(Vec::new()));
    let escapes = Rc::new(Cell::new(0));
    {
        let cleared = Rc::clone(&cleared);
        data.on_clear_refs(move || cleared.set(cleared.get() + 1));
        let selecting = Rc::clone(&selecting);
        data.on_set_group_selecting(move |active| selecting.borrow_mut().push(active));
        let escapes = Rc::clone(&escapes);
        ui.on_escape(move || escapes.set(escapes.get() + 1));
    }

    data.set_ref_count(2);
    data.set_selecting(true);
    refresh(&data);
    press_escape(&ui);
    assert_eq!(cleared.get(), 1, "先清条目选择");
    assert!(selecting.borrow().is_empty(), "条目还在时不退出分组多选");
    assert_eq!(escapes.get(), 0);

    data.set_ref_count(0);
    press_escape(&ui);
    assert_eq!(selecting.borrow().as_slice(), &[false]);
    assert_eq!(escapes.get(), 0);
}

#[test]
fn right_click_opens_a_menu_that_can_be_hit_and_dismissed() {
    let ui = open();
    let data = ui.global::<ShelvesPageData>();
    let menus = Rc::new(RefCell::new(Vec::new()));
    let activated = Rc::new(RefCell::new(Vec::new()));
    let closed = Rc::new(Cell::new(0));
    {
        let menus = Rc::clone(&menus);
        data.on_ref_menu(move |index, x, y| menus.borrow_mut().push((index, x, y)));
        let activated = Rc::clone(&activated);
        data.on_menu_activate(move |index| activated.borrow_mut().push(index));
        let closed = Rc::clone(&closed);
        data.on_close_menu(move || closed.set(closed.get() + 1));
    }

    let rx = data.get_refs_x() + 24.0;
    let item0 = data.get_refs_y() + REF_ROW / 2.0;
    stroke(&ui, (rx, item0), (rx, item0), PointerEventButton::Right);
    assert_eq!(menus.borrow().len(), 1, "行内右键打开菜单");
    assert_eq!(menus.borrow()[0].0, 0);

    data.set_menu_rows(ModelRc::new(VecModel::from(vec![
        menu_row("打开", true),
        menu_row("移动到分组", false),
        menu_row("资料", true),
    ])));
    data.set_menu_x(300.0);
    data.set_menu_y(100.0);
    data.set_menu_open(true);
    slint::platform::update_timers_and_animations();

    let page_y = data.get_page_y();
    assert!(page_y > 40.0, "页面在标题栏下面");
    // 菜单相对收纳页。第一项高 32，内边距 4。
    click(&ui, 300.0 + 40.0, page_y + 100.0 + 4.0 + 16.0);
    assert_eq!(activated.borrow().as_slice(), &[0]);

    data.set_menu_open(true);
    slint::platform::update_timers_and_animations();
    click(&ui, 300.0 + 40.0, page_y + 100.0 + 4.0 + 32.0 + 16.0);
    assert_eq!(activated.borrow().as_slice(), &[0], "禁用行不触发");

    click(&ui, 24.0, page_y + 80.0);
    assert_eq!(closed.get(), 1, "点菜单外关闭");
}
