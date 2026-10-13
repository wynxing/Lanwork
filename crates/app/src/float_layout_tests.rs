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

use lanwork_core::panel::{HANDLE_LENGTH_LOGICAL, HANDLE_THICKNESS_LOGICAL};

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
    assert_handle_box(&note, "left", "便签悬浮窗");
    click(
        &note,
        note.get_strip_x() + note.get_strip_width() / 2.0,
        note.get_strip_y() + note.get_strip_height() / 2.0,
    );
    assert_eq!(strips.get(), 1, "便签悬浮窗：点贴边把手");
    click(
        &note,
        note.get_close_x() + CLOSE_SIZE / 2.0,
        CLOSE_SIZE / 2.0,
    );
    assert_eq!((strips.get(), closes.get()), (1, 1), "点关闭不会点到把手");

    note.set_dock_edge("top".into());
    slint::platform::update_timers_and_animations();
    assert_handle_box(&note, "top", "便签悬浮窗");
    assert!(
        note.get_strip_x() + note.get_strip_width() <= note.get_close_x(),
        "上沿把手不盖住关闭"
    );

    let shelf = ShelfFloat::new().expect("shelf float");
    let strips = Rc::new(Cell::new(0));
    shelf.on_strip_clicked({
        let strips = Rc::clone(&strips);
        move || strips.set(strips.get() + 1)
    });
    shelf.set_dock_edge("left".into());
    shelf.show().expect("show");
    slint::platform::update_timers_and_animations();
    assert_handle_box(&shelf, "left", "收纳悬浮窗");
    click(
        &shelf,
        shelf.get_strip_x() + shelf.get_strip_width() / 2.0,
        shelf.get_strip_y() + shelf.get_strip_height() / 2.0,
    );
    assert_eq!(strips.get(), 1, "收纳悬浮窗：点贴边把手");
}

fn logical(px: u32) -> f32 {
    let px = u16::try_from(px).expect("logical px fits");
    f32::from(px)
}

fn assert_handle_box(ui: &impl HandleBox, edge: &str, label: &str) {
    let vertical = edge != "top";
    let (grip_w, grip_h) = if vertical {
        (
            logical(HANDLE_THICKNESS_LOGICAL),
            logical(HANDLE_LENGTH_LOGICAL),
        )
    } else {
        (
            logical(HANDLE_LENGTH_LOGICAL),
            logical(HANDLE_THICKNESS_LOGICAL),
        )
    };
    assert_eq!(ui.strip_width(), grip_w, "{label}：把手宽");
    assert_eq!(ui.strip_height(), grip_h, "{label}：把手高");
    assert_eq!(ui.grip_width(), grip_w, "{label}：把手宽");
    assert_eq!(ui.grip_height(), grip_h, "{label}：把手高");
    let window_w = 320.0;
    #[allow(clippy::cast_precision_loss)]
    let window_h = WINDOW_HEIGHT as f32;
    let expected_x = if edge == "right" {
        window_w - grip_w
    } else if edge == "top" {
        (window_w - grip_w) / 2.0
    } else {
        0.0
    };
    let expected_y = if edge == "top" {
        0.0
    } else {
        (window_h - grip_h) / 2.0
    };
    assert_eq!(ui.strip_x(), expected_x, "{label}：把手沿边居中");
    assert_eq!(ui.strip_y(), expected_y, "{label}：把手沿边居中");
}

trait HandleBox {
    fn strip_x(&self) -> f32;
    fn strip_y(&self) -> f32;
    fn strip_width(&self) -> f32;
    fn strip_height(&self) -> f32;
    fn grip_width(&self) -> f32;
    fn grip_height(&self) -> f32;
}

impl HandleBox for NoteFloat {
    fn strip_x(&self) -> f32 {
        self.get_strip_x()
    }
    fn strip_y(&self) -> f32 {
        self.get_strip_y()
    }
    fn strip_width(&self) -> f32 {
        self.get_strip_width()
    }
    fn strip_height(&self) -> f32 {
        self.get_strip_height()
    }
    fn grip_width(&self) -> f32 {
        self.get_grip_width()
    }
    fn grip_height(&self) -> f32 {
        self.get_grip_height()
    }
}

impl HandleBox for ShelfFloat {
    fn strip_x(&self) -> f32 {
        self.get_strip_x()
    }
    fn strip_y(&self) -> f32 {
        self.get_strip_y()
    }
    fn strip_width(&self) -> f32 {
        self.get_strip_width()
    }
    fn strip_height(&self) -> f32 {
        self.get_strip_height()
    }
    fn grip_width(&self) -> f32 {
        self.get_grip_width()
    }
    fn grip_height(&self) -> f32 {
        self.get_grip_height()
    }
}

#[test]
fn a_collapsed_float_paints_only_a_short_gray_handle() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    let size = (320, WINDOW_HEIGHT);
    window.set_size(slint::PhysicalSize::new(size.0, size.1));

    let note = NoteFloat::new().expect("note float");
    note.set_limit_width(1920.0);
    note.set_limit_height(1040.0);
    note.set_dock_edge("top".into());
    note.show().expect("show");
    slint::platform::update_timers_and_animations();
    note.set_thin(true);
    note.set_collapsed(true);
    note.set_acrylic(false);
    slint::platform::update_timers_and_animations();
    assert_short_gray_handle(&window, &note, NoteFloat::set_dark, size, "便签悬浮窗");

    let shelf = ShelfFloat::new().expect("shelf float");
    shelf.set_limit_width(1920.0);
    shelf.set_limit_height(1040.0);
    shelf.set_dock_edge("top".into());
    shelf.show().expect("show");
    slint::platform::update_timers_and_animations();
    shelf.set_thin(true);
    shelf.set_collapsed(true);
    shelf.set_acrylic(false);
    slint::platform::update_timers_and_animations();
    assert_short_gray_handle(&window, &shelf, ShelfFloat::set_dark, size, "收纳悬浮窗");

    shelf.set_stack_pages(3);
    shelf.set_stack_index(1);
    slint::platform::update_timers_and_animations();
    let dotted = render_pixels(&window, size);
    let (width, height) = size;
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0;
    let mut max_y = 0;
    let mut bright = 0u32;
    for y in 0..height {
        for x in 0..width {
            let pixel = dotted[(y * width + x) as usize];
            if pixel == 0 {
                continue;
            }
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
            if luminance(pixel) > 80 {
                bright += 1;
            }
        }
    }
    let span_x = max_x - min_x + 1;
    let span_y = max_y - min_y + 1;
    assert!(
        span_x <= HANDLE_LENGTH_LOGICAL + 4 && span_y <= HANDLE_THICKNESS_LOGICAL + 4,
        "叠页的点仍在大约 40×3 的把手里，实际 {span_x}×{span_y}"
    );
    assert!(bright > 0, "叠页把手上有更亮的点");
}

fn render_pixels(window: &Rc<MinimalSoftwareWindow>, size: (u32, u32)) -> Vec<u16> {
    use slint::platform::software_renderer::Rgb565Pixel;

    let (width, height) = size;
    let mut buffer = vec![Rgb565Pixel(0); (width * height) as usize];
    window.request_redraw();
    let drawn = window.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, width as usize);
    });
    assert!(drawn, "窗口画出来了");
    buffer.into_iter().map(|pixel| pixel.0).collect()
}

fn luminance(pixel: u16) -> u32 {
    let red = u32::from((pixel >> 11) & 0x1f);
    let green = u32::from((pixel >> 5) & 0x3f);
    let blue = u32::from(pixel & 0x1f);
    red * 2 + green + blue * 2
}

fn is_gray(pixel: u16) -> bool {
    let red = i32::from((pixel >> 11) & 0x1f);
    let blue = i32::from(pixel & 0x1f);
    (red - blue).abs() <= 3
}

fn assert_short_gray_handle<C: ComponentHandle>(
    window: &Rc<MinimalSoftwareWindow>,
    ui: &C,
    set_dark: impl Fn(&C, bool),
    size: (u32, u32),
    label: &str,
) {
    let pixels = render_pixels(window, size);
    let (width, height) = size;
    let mut min_x = width;
    let mut min_y = height;
    let mut max_x = 0;
    let mut max_y = 0;
    let mut count = 0u32;
    for y in 0..height {
        for x in 0..width {
            let pixel = pixels[(y * width + x) as usize];
            if pixel == 0 {
                continue;
            }
            count += 1;
            min_x = min_x.min(x);
            min_y = min_y.min(y);
            max_x = max_x.max(x);
            max_y = max_y.max(y);
            assert!(is_gray(pixel), "{label}：把手是灰的，不是强调色");
        }
    }
    assert!(count > 0, "{label}：把手有像素");
    let span_x = max_x - min_x + 1;
    let span_y = max_y - min_y + 1;
    assert!(
        span_x <= HANDLE_LENGTH_LOGICAL + 4 && span_y <= HANDLE_THICKNESS_LOGICAL + 4,
        "{label}：看得见的只有大约 40×3 的把手，实际 {span_x}×{span_y}"
    );
    assert!(
        span_x + 8 < width && span_y + 8 < height,
        "{label}：没有铺满窗口的色条"
    );
    let center_x = (min_x + max_x) / 2;
    let center_y = (min_y + max_y) / 2;
    let rest = luminance(pixels[(center_y * width + center_x) as usize]);

    set_dark(ui, true);
    slint::platform::update_timers_and_animations();
    let dark = render_pixels(window, size);
    let dark_pixel = dark[(center_y * width + center_x) as usize];
    assert!(is_gray(dark_pixel), "{label}：暗色主题仍是灰");
    assert!(luminance(dark_pixel) > rest, "{label}：暗色主题的灰更浅");
    set_dark(ui, false);
    slint::platform::update_timers_and_animations();

    #[allow(clippy::cast_precision_loss)]
    let center = slint::LogicalPosition::new(center_x as f32, center_y as f32);
    ui.window()
        .dispatch_event(WindowEvent::PointerMoved { position: center });
    slint::platform::update_timers_and_animations();
    let hovered = render_pixels(window, size);
    let hot = luminance(hovered[(center_y * width + center_x) as usize]);
    assert!(hot > rest, "{label}：鼠标靠近时把手更亮");
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
