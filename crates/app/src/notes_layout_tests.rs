//! 便签页多选的布局和点击。软件渲染摆出面板的便签标签，确认「选择」、勾选和底部操作条互不重叠，
//! 点得到，Esc 退出多选时不收起面板。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{NoteListRow, NotesPageData, Panel};

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn row(title: &str, checked: bool) -> NoteListRow {
    NoteListRow {
        title: title.into(),
        pinned: false,
        tags: "工作".into(),
        note: "".into(),
        checked,
    }
}

fn open() -> Panel {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    window.set_size(slint::PhysicalSize::new(760, 560));
    let ui = Panel::new().expect("panel");
    ui.set_dark(false);
    ui.set_tab(1);
    let data = ui.global::<NotesPageData>();
    data.set_rows(ModelRc::new(VecModel::from(vec![
        row("采购", false),
        row("会议", false),
        row("草稿", true),
    ])));
    data.set_trash(false);
    data.set_selecting(false);
    data.set_selected_count(0);
    data.set_confirm_delete(false);
    data.set_list_count(3);
    ui.show().expect("show");
    slint::platform::update_timers_and_animations();
    let data = ui.global::<NotesPageData>();
    data.set_geometry_refresh(data.get_geometry_refresh().wrapping_add(1));
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

fn press_escape(ui: &Panel) {
    ui.invoke_focus_root();
    ui.window().dispatch_event(WindowEvent::KeyPressed {
        text: "\u{001b}".into(),
    });
}

/// 便签页在标题栏和分隔线下面。几何存在页面自己的坐标里，点击要加上这段。
const PAGE_Y: f32 = 49.0;

struct Geom {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

fn choose_box(data: &NotesPageData) -> Geom {
    Geom {
        x: data.get_choose_x(),
        y: data.get_choose_y() + PAGE_Y,
        w: data.get_choose_w(),
        h: data.get_choose_h(),
    }
}

#[test]
fn select_mode_puts_the_bar_under_the_list_and_escape_does_not_hide_the_panel() {
    let ui = open();
    let data = ui.global::<NotesPageData>();
    let selecting = Rc::new(RefCell::new(Vec::new()));
    let rows = Rc::new(RefCell::new(Vec::new()));
    let asked = Rc::new(Cell::new(0));
    let escapes = Rc::new(Cell::new(0));
    {
        let selecting = Rc::clone(&selecting);
        data.on_set_selecting(move |active| selecting.borrow_mut().push(active));
        let rows = Rc::clone(&rows);
        data.on_row_clicked(move |index, ctrl, shift| {
            rows.borrow_mut().push((index, ctrl, shift));
        });
        let asked = Rc::clone(&asked);
        data.on_ask_delete(move || asked.set(asked.get() + 1));
        let escapes = Rc::clone(&escapes);
        ui.on_escape(move || escapes.set(escapes.get() + 1));
    }

    let choose = choose_box(&data);
    let list_y = data.get_list_y() + PAGE_Y;
    let list_h = data.get_list_h();
    let new_y = data.get_new_y() + PAGE_Y;
    assert!(choose.h > 0.0, "选择按钮有高度");
    assert!(list_h > 0.0, "列表有高度");
    assert!(choose.y + choose.h <= list_y, "选择在列表上方");
    assert!(
        list_y + list_h <= new_y + 1.0,
        "未多选时操作条不占列表和新建之间的位置"
    );
    assert_eq!(data.get_count_label().as_str(), "已选 0 条");

    click(&ui, choose.x + choose.w / 2.0, choose.y + choose.h / 2.0);
    assert_eq!(*selecting.borrow(), [true]);

    data.set_selecting(true);
    data.set_selected_count(1);
    data.set_geometry_refresh(data.get_geometry_refresh().wrapping_add(1));
    slint::platform::update_timers_and_animations();

    let list_y = data.get_list_y() + PAGE_Y;
    let list_h = data.get_list_h();
    let bar_y = data.get_bar_y() + PAGE_Y;
    let bar_h = data.get_bar_h();
    let new_y = data.get_new_y() + PAGE_Y;
    assert!(bar_h > 40.0, "操作条有计数和按钮");
    assert!(
        list_y + list_h <= bar_y + 1.0,
        "列表不压住操作条 {list_y}+{list_h} vs {bar_y}"
    );
    assert!(
        bar_y + bar_h <= new_y + 1.0,
        "操作条不压住新建 {bar_y}+{bar_h} vs {new_y}"
    );
    assert_eq!(data.get_count_label().as_str(), "已选 1 条");

    click(&ui, 20.0, list_y + 26.0);
    assert_eq!(rows.borrow().as_slice(), &[(0, false, false)]);

    click(
        &ui,
        data.get_delete_x() + data.get_delete_w() / 2.0,
        data.get_delete_y() + PAGE_Y + data.get_delete_h() / 2.0,
    );
    assert_eq!(asked.get(), 1, "删除先进入确认，不直接删");

    press_escape(&ui);
    assert_eq!(selecting.borrow().as_slice(), &[true, false]);
    assert_eq!(escapes.get(), 0, "多选时 Esc 不收起面板");

    data.set_selecting(false);
    data.set_geometry_refresh(data.get_geometry_refresh().wrapping_add(1));
    slint::platform::update_timers_and_animations();
    press_escape(&ui);
    assert_eq!(escapes.get(), 1, "退出多选之后 Esc 仍收起面板");
}

#[test]
fn trash_has_no_select_button() {
    let ui = open();
    let data = ui.global::<NotesPageData>();
    data.set_trash(true);
    data.set_selecting(true);
    data.set_geometry_refresh(data.get_geometry_refresh().wrapping_add(1));
    slint::platform::update_timers_and_animations();
    assert_eq!(data.get_choose_h(), 0.0, "回收站不显示选择");
    assert_eq!(data.get_bar_h(), 0.0, "回收站不显示操作条");
}
