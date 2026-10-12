//! 收纳悬浮窗的布局检查。用软件渲染的窗口把 `ShelfFloat` 摆出来，读它导出的各块位置，
//! 确认「关联待办」标题行、关联列表、条目列表和底部操作行在展开、收起、不同条数下互不重叠。
//!
//! 起因：标题行当时没写 `y`，Slint 会把没有 `x`、`y` 的子元素放在父元素正中，
//! 展开后父元素变高，标题行被推到列表的第一行上。

use std::rc::Rc;

use std::cell::Cell;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{NoteFloat, ShelfFloat, ShelfLinkRow, ShelfRefRow, ShelvesPageData};

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

const WINDOW_HEIGHT: u32 = 360;
const TITLE_STRIP: f32 = 28.0;
const HEAD: f32 = 28.0;
const ROW: f32 = 30.0;
const MAX_ROWS: usize = 4;

fn links(count: usize) -> Vec<ShelfLinkRow> {
    (0..count)
        .map(|index| ShelfLinkRow {
            title: format!("待办 {index}").into(),
            found: true,
            completed: index % 2 == 0,
            in_trash: false,
        })
        .collect()
}

fn lay_out(window: &Rc<MinimalSoftwareWindow>, open: bool, count: usize, selected: bool) {
    let ui = ShelfFloat::new().expect("window");
    window.set_size(slint::PhysicalSize::new(320, WINDOW_HEIGHT));
    let data = ui.global::<ShelvesPageData>();
    data.set_links(ModelRc::new(VecModel::from(links(count))));
    data.set_links_open(open);
    data.set_refs(ModelRc::new(VecModel::from(vec![ShelfRefRow {
        name: "你好.txt".into(),
        icon: slint::Image::default(),
        has_icon: false,
        missing: false,
    }])));
    data.set_selected_ref(if selected { 0 } else { -1 });
    ui.show().expect("show");
    slint::platform::update_timers_and_animations();

    let label = format!("open={open} links={count} selected={selected}");
    let (head_y, head_h) = (ui.get_links_head_y(), ui.get_links_head_height());
    let (body_y, body_h) = (ui.get_links_body_y(), ui.get_links_body_height());
    let (refs_y, refs_h) = (ui.get_refs_area_y(), ui.get_refs_area_height());
    let action_y = ui.get_action_y();

    assert_eq!(head_y, TITLE_STRIP, "{label}: 标题行紧挨着窗口上沿的拖动条");
    assert_eq!(head_h, HEAD, "{label}");
    assert_eq!(body_y, head_y + head_h, "{label}: 关联列表紧接在标题行下面");
    let rows = if open { count.clamp(1, MAX_ROWS) } else { 0 };
    #[allow(clippy::cast_precision_loss)]
    let expected_body = rows as f32 * ROW;
    assert_eq!(body_h, expected_body, "{label}: 关联列表高度");
    assert_eq!(
        refs_y,
        body_y + body_h,
        "{label}: 条目列表紧接在关联列表下面"
    );
    assert!(refs_h > 0.0, "{label}: 条目列表还有空间");
    if selected {
        assert!(
            refs_y + refs_h <= action_y,
            "{label}: 条目列表不压到底部操作行 ({} > {action_y})",
            refs_y + refs_h
        );
    }
    #[allow(clippy::cast_precision_loss)]
    let bottom = WINDOW_HEIGHT as f32;
    assert!(refs_y + refs_h <= bottom, "{label}: 不超出窗口");
}

#[test]
fn the_float_window_blocks_never_overlap() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    for open in [false, true] {
        for count in [0, 1, 2, 4, 5, 9] {
            for selected in [false, true] {
                lay_out(&window, open, count, selected);
            }
        }
    }
}

const CLOSE_SIZE: f32 = 28.0;

/// 把窗口画进缓冲区，取出一块像素。
fn render_region(
    window: &Rc<MinimalSoftwareWindow>,
    size: (u32, u32),
    region: (u32, u32, u32, u32),
) -> Vec<u8> {
    use slint::platform::software_renderer::Rgb565Pixel;

    let (width, height) = size;
    let mut buffer = vec![Rgb565Pixel(0); (width * height) as usize];
    window.request_redraw();
    let drawn = window.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, width as usize);
    });
    assert!(drawn, "窗口画出来了");
    let (x0, y0, w, h) = region;
    let mut out = Vec::new();
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            out.extend_from_slice(&buffer[(y * width + x) as usize].0.to_le_bytes());
        }
    }
    out
}

fn click(ui: &impl ComponentHandle, x: f32, y: f32) {
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

#[test]
fn the_close_button_stays_at_the_right_of_the_title_strip() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    window.set_size(slint::PhysicalSize::new(320, WINDOW_HEIGHT));

    let note = NoteFloat::new().expect("note float");
    note.show().expect("show");
    slint::platform::update_timers_and_animations();
    assert_eq!(
        note.get_close_x(),
        320.0 - CLOSE_SIZE,
        "便签悬浮窗：关闭在右上"
    );
    assert_eq!(note.get_close_y(), 0.0);

    let shelf = ShelfFloat::new().expect("shelf float");
    shelf.show().expect("show");
    slint::platform::update_timers_and_animations();
    assert_eq!(
        shelf.get_close_x(),
        320.0 - CLOSE_SIZE,
        "收纳悬浮窗：关闭在右上"
    );
}

#[test]
fn clicking_the_left_edge_strip_asks_to_collapse_and_does_not_close() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    window.set_size(slint::PhysicalSize::new(320, WINDOW_HEIGHT));

    let note = NoteFloat::new().expect("note float");
    let strips = Rc::new(Cell::new(0));
    note.on_strip_clicked({
        let strips = Rc::clone(&strips);
        move || strips.set(strips.get() + 1)
    });
    let closes = Rc::new(Cell::new(0));
    note.on_close_clicked({
        let closes = Rc::clone(&closes);
        move || closes.set(closes.get() + 1)
    });
    note.set_dock_edge("left".into());
    note.show().expect("show");
    slint::platform::update_timers_and_animations();
    assert_eq!(note.get_strip_x(), 0.0);
    assert!(note.get_strip_width() > 0.0 && note.get_strip_width() < CLOSE_SIZE);
    click(&note, note.get_strip_width() / 2.0, 40.0);
    assert_eq!(strips.get(), 1, "便签悬浮窗：点贴边细条");
    click(
        &note,
        note.get_close_x() + CLOSE_SIZE / 2.0,
        CLOSE_SIZE / 2.0,
    );
    assert_eq!((strips.get(), closes.get()), (1, 1), "点关闭不会点到细条");

    let shelf = ShelfFloat::new().expect("shelf float");
    let strips = Rc::new(Cell::new(0));
    shelf.on_strip_clicked({
        let strips = Rc::clone(&strips);
        move || strips.set(strips.get() + 1)
    });
    shelf.set_dock_edge("left".into());
    shelf.show().expect("show");
    slint::platform::update_timers_and_animations();
    click(&shelf, shelf.get_strip_width() / 2.0, 40.0);
    assert_eq!(strips.get(), 1, "收纳悬浮窗：点贴边细条");
}

#[test]
fn a_collapsed_float_paints_the_accent_strip_instead_of_the_body() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    let size = (320, WINDOW_HEIGHT);
    window.set_size(slint::PhysicalSize::new(size.0, size.1));

    let note = NoteFloat::new().expect("note float");
    note.show().expect("show");
    slint::platform::update_timers_and_animations();
    let open = render_region(&window, size, (8, 40, 12, 12));
    note.set_thin(true);
    note.set_collapsed(true);
    slint::platform::update_timers_and_animations();
    let collapsed = render_region(&window, size, (8, 40, 12, 12));
    assert_ne!(open, collapsed, "便签悬浮窗：收起后不再是正文");
    assert!(
        collapsed.iter().any(|byte| *byte != 0),
        "便签悬浮窗：细条不是一块空白"
    );

    let shelf = ShelfFloat::new().expect("shelf float");
    shelf.show().expect("show");
    slint::platform::update_timers_and_animations();
    let open = render_region(&window, size, (8, 40, 12, 12));
    shelf.set_thin(true);
    shelf.set_collapsed(true);
    slint::platform::update_timers_and_animations();
    let collapsed = render_region(&window, size, (8, 40, 12, 12));
    assert_ne!(open, collapsed, "收纳悬浮窗：收起后不再是条目列表");
    assert!(
        collapsed.iter().any(|byte| *byte != 0),
        "收纳悬浮窗：细条不是一块空白"
    );
}

#[test]
fn both_float_windows_are_always_on_top_and_have_no_pin_button() {
    for (name, source) in [
        ("notefloat.slint", include_str!("../ui/notefloat.slint")),
        ("shelffloat.slint", include_str!("../ui/shelffloat.slint")),
    ] {
        assert!(
            source.contains("always-on-top: true;"),
            "{name} 没有始终置顶"
        );
        assert!(!source.contains("PinBtn"), "{name} 还有钉住按钮");
        assert!(!source.contains("pin-clicked"), "{name} 还有钉住回调");
    }
    let widgets = include_str!("../ui/widgets.slint");
    assert!(!widgets.contains("PinBtn"), "共用控件里还有钉住按钮");
}
