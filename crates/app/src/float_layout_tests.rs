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

const PIN_SIZE: f32 = 28.0;

/// 把窗口画进缓冲区，取出钉住按钮那一块的像素。
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

fn assert_pin_in_title_strip(label: &str, x: f32, y: f32, w: f32, h: f32, close_x: f32) {
    assert_eq!((w, h), (PIN_SIZE, PIN_SIZE), "{label}: 钉住按钮的大小");
    assert_eq!(y, 0.0, "{label}: 钉住按钮在窗口上沿的标题条里");
    assert!(
        x + w <= close_x,
        "{label}: 钉住按钮在关闭按钮左边，不压住它 ({x}+{w} > {close_x})"
    );
    assert_eq!(x + w, close_x, "{label}: 钉住按钮紧挨着关闭按钮");
}

#[test]
fn the_pin_button_sits_left_of_close_and_shows_whether_it_is_pinned() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    window.set_size(slint::PhysicalSize::new(320, WINDOW_HEIGHT));

    let note = NoteFloat::new().expect("note float");
    note.show().expect("show");
    slint::platform::update_timers_and_animations();
    assert_pin_in_title_strip(
        "便签悬浮窗",
        note.get_pin_x(),
        note.get_pin_y(),
        note.get_pin_width(),
        note.get_pin_height(),
        note.get_close_x(),
    );
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let region = (note.get_pin_x() as u32, 0, PIN_SIZE as u32, PIN_SIZE as u32);
    let size = (320, WINDOW_HEIGHT);
    note.set_pinned(false);
    let unpinned = render_region(&window, size, region);
    note.set_pinned(true);
    let pinned = render_region(&window, size, region);
    assert_ne!(unpinned, pinned, "便签悬浮窗：钉住前后按钮看起来不同");
    note.set_pinned(false);
    assert_eq!(
        unpinned,
        render_region(&window, size, region),
        "便签悬浮窗：取消钉住后按钮恢复原样"
    );

    let shelf = ShelfFloat::new().expect("shelf float");
    shelf.show().expect("show");
    slint::platform::update_timers_and_animations();
    assert_pin_in_title_strip(
        "收纳悬浮窗",
        shelf.get_pin_x(),
        shelf.get_pin_y(),
        shelf.get_pin_width(),
        shelf.get_pin_height(),
        shelf.get_close_x(),
    );
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let region = (
        shelf.get_pin_x() as u32,
        0,
        PIN_SIZE as u32,
        PIN_SIZE as u32,
    );
    shelf.set_pinned(false);
    let unpinned = render_region(&window, size, region);
    shelf.set_pinned(true);
    let pinned = render_region(&window, size, region);
    assert_ne!(unpinned, pinned, "收纳悬浮窗：钉住前后按钮看起来不同");
}

#[test]
fn clicking_the_pin_button_asks_to_toggle_and_the_window_keeps_the_pinned_value_it_was_given() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    window.set_size(slint::PhysicalSize::new(320, WINDOW_HEIGHT));

    let note = NoteFloat::new().expect("note float");
    let clicks = Rc::new(Cell::new(0));
    note.on_pin_clicked({
        let clicks = Rc::clone(&clicks);
        move || clicks.set(clicks.get() + 1)
    });
    let closes = Rc::new(Cell::new(0));
    note.on_close_clicked({
        let closes = Rc::clone(&closes);
        move || closes.set(closes.get() + 1)
    });
    note.show().expect("show");
    slint::platform::update_timers_and_animations();
    let (x, y) = (note.get_pin_x() + PIN_SIZE / 2.0, PIN_SIZE / 2.0);
    click(&note, x, y);
    assert_eq!(clicks.get(), 1, "便签悬浮窗：点一下问一次");
    assert!(
        !note.get_pinned(),
        "便签悬浮窗：状态由 Rust 在写盘成功后改，点击本身不改"
    );
    assert_eq!(closes.get(), 0, "点钉住不会关窗口");
    click(&note, note.get_close_x() + PIN_SIZE / 2.0, PIN_SIZE / 2.0);
    assert_eq!((clicks.get(), closes.get()), (1, 1), "点关闭不会触发钉住");

    let shelf = ShelfFloat::new().expect("shelf float");
    let clicks = Rc::new(Cell::new(0));
    shelf.on_pin_clicked({
        let clicks = Rc::clone(&clicks);
        move || clicks.set(clicks.get() + 1)
    });
    shelf.show().expect("show");
    slint::platform::update_timers_and_animations();
    click(&shelf, shelf.get_pin_x() + PIN_SIZE / 2.0, PIN_SIZE / 2.0);
    assert_eq!(clicks.get(), 1, "收纳悬浮窗：点一下问一次");
    assert!(!shelf.get_pinned());
}

#[test]
fn both_float_windows_tie_always_on_top_to_the_pinned_property() {
    for (name, source) in [
        ("notefloat.slint", include_str!("../ui/notefloat.slint")),
        ("shelffloat.slint", include_str!("../ui/shelffloat.slint")),
    ] {
        assert!(
            source.contains("always-on-top: pinned;"),
            "{name} 没有把 always-on-top 接到 pinned"
        );
    }
}
