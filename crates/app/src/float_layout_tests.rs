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

use lanwork_core::panel::{
    DOT_DIAMETER_LOGICAL, DOT_GAP_LOGICAL, HANDLE_LENGTH_LOGICAL, HANDLE_THICKNESS_LOGICAL,
};

use crate::{
    FloatShell, FloatShellProbe, NoteFloat, ShelfFloat, ShelfLinkRow, ShelfRefRow, ShelvesPageData,
    StackTab, TodoFloat,
};

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

const WINDOW_HEIGHT: u32 = 360;
const TITLE_STRIP: f32 = 28.0;
const SOURCE_ROW: f32 = 20.0;
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
        selected: false,
    }])));
    data.set_selected_ref(if selected { 0 } else { -1 });
    ui.show().expect("show");
    slint::platform::update_timers_and_animations();

    let label = format!("open={open} links={count} selected={selected}");
    let (head_y, head_h) = (ui.get_links_head_y(), ui.get_links_head_height());
    let (body_y, body_h) = (ui.get_links_body_y(), ui.get_links_body_height());
    let (refs_y, refs_h) = (ui.get_refs_area_y(), ui.get_refs_area_height());
    let action_y = ui.get_action_y();

    assert_eq!(
        ui.get_source_y(),
        TITLE_STRIP,
        "{label}: 来源行紧挨着拖动条"
    );
    assert_eq!(ui.get_source_h(), SOURCE_ROW, "{label}");
    assert!(ui.get_source_open(), "{label}: 展开时显示来源行");
    assert_eq!(
        head_y,
        TITLE_STRIP + SOURCE_ROW,
        "{label}: 「关联待办」紧挨着来源行"
    );
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

impl HandleBox for TodoFloat {
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

    let todo = TodoFloat::new().expect("todo float");
    todo.set_limit_width(1920.0);
    todo.set_limit_height(1040.0);
    todo.set_dock_edge("top".into());
    todo.show().expect("show");
    slint::platform::update_timers_and_animations();
    todo.set_thin(true);
    todo.set_collapsed(true);
    todo.set_acrylic(false);
    slint::platform::update_timers_and_animations();
    assert_short_gray_handle(&window, &todo, TodoFloat::set_dark, size, "待办悬浮窗");
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
fn source_line_is_a_single_quiet_row_and_each_page_keeps_its_own() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    window.set_size(slint::PhysicalSize::new(320, WINDOW_HEIGHT));
    let long = "收纳 · 这是一个长到放不进悬浮窗的分组名，界面应在行尾显示省略号而不是换行";

    let note = NoteFloat::new().expect("note");
    note.set_source(long.into());
    note.show().expect("show");
    slint::platform::update_timers_and_animations();
    assert_eq!(note.get_source().as_str(), long, "名称不在数据里截断");
    assert!(!note.get_pin_drawn(), "便签悬浮窗没有钉住按钮");
    assert!(note.get_above_ordinary(), "便签悬浮窗始终在最上层");
    assert_eq!(note.get_source_y(), TITLE_STRIP);
    assert_eq!(note.get_source_h(), SOURCE_ROW);
    assert_eq!(note.get_body_y(), TITLE_STRIP + SOURCE_ROW);
    assert_eq!(note.get_close_y(), 0.0);
    assert!(note.get_source_y() >= note.get_close_y() + CLOSE_SIZE);

    let shelf = ShelfFloat::new().expect("shelf");
    shelf.set_source("收纳 · 单片机".into());
    shelf.show().expect("show");
    slint::platform::update_timers_and_animations();
    assert_eq!(shelf.get_source().as_str(), "收纳 · 单片机");
    assert_eq!(shelf.get_source_y(), TITLE_STRIP);
    assert_eq!(shelf.get_links_head_y(), TITLE_STRIP + SOURCE_ROW);

    let todo = TodoFloat::new().expect("todo");
    todo.show().expect("show");
    slint::platform::update_timers_and_animations();
    assert_eq!(todo.get_source().as_str(), "待办");
    assert_ne!(note.get_source().as_str(), shelf.get_source().as_str());
    assert_ne!(shelf.get_source().as_str(), todo.get_source().as_str());

    note.set_collapsed(true);
    slint::platform::update_timers_and_animations();
    assert!(!note.get_source_open(), "收起的把手不显示来源行");

    let shell = include_str!("../ui/float_shell.slint");
    assert!(
        shell.contains("source-row := FloatSource"),
        "来源行画在共用外壳上"
    );
    assert!(shell.contains("y: 28px;"), "来源行应在拖动条下面");
    assert!(
        shell.contains("visible: root.source-band > 0px;"),
        "收起或工具窗口不显示来源行"
    );
    for (name, source) in [
        ("notefloat.slint", include_str!("../ui/notefloat.slint")),
        ("shelffloat.slint", include_str!("../ui/shelffloat.slint")),
        ("todofloat.slint", include_str!("../ui/todofloat.slint")),
    ] {
        assert!(
            source.contains("chrome := FloatChrome"),
            "{name} 应把内容放进共用外壳"
        );
        assert!(source.contains("show-source: true;"), "{name} 应打开来源行");
    }
    let group = include_str!("float_group.rs");
    assert!(group.contains("note_float::present"));
    assert!(group.contains("shelf_float::present"));
    assert!(group.contains("todo_float::present"));
    assert!(
        group.contains("apply_clear_plate"),
        "叠页页点应清掉会在失焦时变不透明的系统背景"
    );

    let widgets = include_str!("../ui/widgets.slint");
    let source = widgets
        .split("export component FloatSource")
        .nth(1)
        .expect("FloatSource");
    assert!(source.contains("overflow: elide;"), "来源行过长应省略");
    assert!(source.contains("font-size: 12px;"));
}

#[test]
fn both_float_windows_are_always_on_top_and_have_no_pin_button() {
    for (name, source) in [
        ("notefloat.slint", include_str!("../ui/notefloat.slint")),
        ("shelffloat.slint", include_str!("../ui/shelffloat.slint")),
        ("todofloat.slint", include_str!("../ui/todofloat.slint")),
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

#[test]
fn todo_float_empty_list_sits_under_the_drag_strip() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    window.set_size(slint::PhysicalSize::new(320, WINDOW_HEIGHT));
    let ui = TodoFloat::new().expect("todo float");
    ui.set_acrylic(false);
    ui.show().expect("show");
    slint::platform::update_timers_and_animations();
    assert!(ui.get_shows_empty(), "没有行时显示空状态");
    assert_eq!(ui.get_source(), "待办");
    assert_eq!(ui.get_source_y(), TITLE_STRIP, "来源行紧挨着拖动条");
    assert_eq!(ui.get_source_h(), SOURCE_ROW);
    assert_eq!(
        ui.get_content_y(),
        TITLE_STRIP + SOURCE_ROW,
        "列表从来源行下面开始"
    );
    assert_eq!(ui.get_row_h(), 36.0);
}

fn circle_origin(index: u32) -> u32 {
    index * (DOT_DIAMETER_LOGICAL + DOT_GAP_LOGICAL)
}

fn row_span(pages: u32) -> u32 {
    let pages = pages.max(1);
    DOT_DIAMETER_LOGICAL * pages + DOT_GAP_LOGICAL * pages.saturating_sub(1)
}

struct Rgb {
    r: i32,
    g: i32,
    b: i32,
}

fn rgb(pixel: u16) -> Rgb {
    Rgb {
        r: i32::from((pixel >> 11) & 0x1f),
        g: i32::from((pixel >> 5) & 0x3f),
        b: i32::from(pixel & 0x1f),
    }
}

fn describe(pixel: u16) -> String {
    let color = rgb(pixel);
    format!("r{} g{} b{}", color.r, color.g, color.b)
}

fn is_red(pixel: u16) -> bool {
    let color = rgb(pixel);
    color.r >= 8 && color.r > color.b + 4 && color.r * 2 > color.g
}

fn is_yellow(pixel: u16) -> bool {
    let color = rgb(pixel);
    color.r >= 8 && color.g >= 12 && color.b + 6 < color.r && color.g > color.b * 2
}

fn is_green(pixel: u16) -> bool {
    let color = rgb(pixel);
    color.g >= 12 && color.g > color.r * 2 && color.g > color.b * 2
}

fn is_light_ring(pixel: u16) -> bool {
    let color = rgb(pixel);
    color.r >= 24 && color.g >= 40 && color.b >= 16
}

fn pixels_in(pixels: &[u16], width: u32, x0: u32, x1: u32) -> impl Iterator<Item = u16> + '_ {
    let height = u32::try_from(pixels.len()).unwrap_or(0) / width.max(1);
    (0..height).flat_map(move |y| {
        (x0..x1).filter_map(move |x| {
            let index = usize::try_from(y.saturating_mul(width).saturating_add(x)).ok()?;
            pixels.get(index).copied()
        })
    })
}

fn ink_count(pixels: &[u16], width: u32, x0: u32, x1: u32) -> u32 {
    pixels_in(pixels, width, x0, x1)
        .filter(|pixel| *pixel != 0)
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

fn dot_center(pixels: &[u16], width: u32, origin: u32) -> u16 {
    let mid = DOT_DIAMETER_LOGICAL / 2;
    let index = usize::try_from(
        mid.saturating_mul(width)
            .saturating_add(origin)
            .saturating_add(mid),
    )
    .unwrap_or(0);
    pixels.get(index).copied().unwrap_or(0)
}

#[test]
fn page_dots_are_soft_colors_left_aligned_and_the_current_one_is_solid() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    let wide = 220u32;
    window.set_size(slint::PhysicalSize::new(wide, DOT_DIAMETER_LOGICAL));

    let ui = StackTab::new().expect("stack tab");
    ui.window()
        .set_size(slint::PhysicalSize::new(wide, DOT_DIAMETER_LOGICAL));
    ui.set_dark(false);
    ui.set_pages(5);
    ui.set_current(0);
    ui.show().expect("show");
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.window().scale_factor(), 1.0);

    let hit = Rc::new(Cell::new(-1));
    let slot = Rc::clone(&hit);
    ui.on_drag_dot(move |index| slot.set(index));

    let pixels = render_pixels(&window, (wide, DOT_DIAMETER_LOGICAL));
    let first = circle_origin(0);
    let second = circle_origin(1);
    let third = circle_origin(2);
    let end = row_span(5);
    assert_eq!(DOT_DIAMETER_LOGICAL, 16);
    assert_eq!(end, 16 * 5 + 4 * 4, "五页一行是 16×5 + 4×4");
    assert!(
        ink_count(&pixels, wide, 0, 4) > 0,
        "第一个圆点贴着这一行的左边缘"
    );
    assert_eq!(
        ink_count(&pixels, wide, end, wide),
        0,
        "圆点靠左排，右侧留空"
    );
    assert_eq!(
        ink_count(&pixels, wide, DOT_DIAMETER_LOGICAL, second),
        0,
        "圆点之间有一段空白间隔"
    );

    let red = dot_center(&pixels, wide, first);
    let yellow = dot_center(&pixels, wide, second);
    let green = dot_center(&pixels, wide, third);
    assert!(is_red(red), "第 1 页是红，实际 {}", describe(red));
    assert!(is_yellow(yellow), "第 2 页是黄，实际 {}", describe(yellow));
    assert!(is_green(green), "第 3 页是绿，实际 {}", describe(green));
    assert!(
        luminance(red) > luminance(yellow) && luminance(red) > luminance(green),
        "当前页比其余页更实"
    );
    assert!(
        pixels_in(&pixels, wide, first, first + DOT_DIAMETER_LOGICAL).any(is_light_ring),
        "当前页边上有浅色细光圈"
    );
    assert!(
        !pixels_in(&pixels, wide, second, second + DOT_DIAMETER_LOGICAL).any(is_light_ring),
        "其余页没有光圈"
    );

    let center_y = logical(DOT_DIAMETER_LOGICAL) / 2.0;
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(logical(second + DOT_DIAMETER_LOGICAL / 2), center_y),
    });
    slint::platform::update_timers_and_animations();
    let hovered = render_pixels(&window, (wide, DOT_DIAMETER_LOGICAL));
    let yellow_hot = dot_center(&hovered, wide, second);
    assert!(
        luminance(yellow_hot) > luminance(yellow),
        "悬停后其余页更亮：{} -> {}",
        describe(yellow),
        describe(yellow_hot)
    );
    assert!(
        luminance(yellow_hot) < luminance(red),
        "悬停后的其余页仍比当前页更透"
    );

    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(-20.0, -20.0),
    });
    slint::platform::update_timers_and_animations();

    for index in 0..5 {
        let x = logical(circle_origin(index) + DOT_DIAMETER_LOGICAL / 2);
        click(&ui, x, center_y);
        assert_eq!(
            hit.get(),
            i32::try_from(index).unwrap(),
            "点第 {} 个圆点",
            index + 1
        );
    }
    click(
        &ui,
        logical(DOT_DIAMETER_LOGICAL + DOT_GAP_LOGICAL / 2),
        center_y,
    );
    assert_eq!(hit.get(), 4, "点在圆点之间的间隔上不换页");

    ui.set_current(1);
    slint::platform::update_timers_and_animations();
    let pixels = render_pixels(&window, (wide, DOT_DIAMETER_LOGICAL));
    let red = dot_center(&pixels, wide, first);
    let yellow = dot_center(&pixels, wide, second);
    assert!(is_yellow(yellow), "当前页换成黄，实际 {}", describe(yellow));
    assert!(luminance(yellow) > luminance(red), "黄比退下去的红更实");

    ui.set_dark(true);
    slint::platform::update_timers_and_animations();
    let pixels = render_pixels(&window, (wide, DOT_DIAMETER_LOGICAL));
    assert!(
        is_yellow(dot_center(&pixels, wide, second)),
        "暗色主题仍用同一套圆点颜色"
    );

    let many = 9u32;
    let span = row_span(many);
    window.set_size(slint::PhysicalSize::new(span, DOT_DIAMETER_LOGICAL));
    ui.window()
        .set_size(slint::PhysicalSize::new(span, DOT_DIAMETER_LOGICAL));
    ui.set_pages(i32::try_from(many).unwrap());
    ui.set_current(0);
    slint::platform::update_timers_and_animations();
    let pixels = render_pixels(&window, (span, DOT_DIAMETER_LOGICAL));
    let again = dot_center(&pixels, span, circle_origin(8));
    assert!(is_red(again), "第 9 页回到红，实际 {}", describe(again));
    click(
        &ui,
        logical(circle_origin(8) + DOT_DIAMETER_LOGICAL / 2),
        center_y,
    );
    assert_eq!(hit.get(), 8, "点第 9 页");
}

#[test]
fn the_tool_shell_has_a_note_style_drag_strip_pin_and_close() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    window.set_size(slint::PhysicalSize::new(320, WINDOW_HEIGHT));

    let shell = FloatShell::new().expect("tool shell");
    shell
        .window()
        .set_size(slint::PhysicalSize::new(320, WINDOW_HEIGHT));
    shell.set_marker("示例".into());
    shell.show().expect("show");
    slint::platform::update_timers_and_animations();

    #[allow(clippy::cast_precision_loss)]
    let height = WINDOW_HEIGHT as f32;
    assert!(shell.get_pinned());
    assert!(shell.get_above_ordinary(), "没记下钉住时在最上层");
    assert!(shell.get_pin_drawn());
    assert!(shell.get_close_drawn());
    assert!(!shell.get_source_drawn(), "不画来源行");
    assert_eq!(shell.get_drag_band(), TITLE_STRIP);
    assert_eq!(shell.get_pin_x(), 320.0 - 56.0);
    assert_eq!(shell.get_pin_y(), 0.0);
    assert_eq!(shell.get_close_x(), 320.0 - 28.0);
    assert_eq!(shell.get_close_y(), 0.0);
    assert!(shell.get_content_open());
    assert_eq!(shell.get_content_y(), TITLE_STRIP);
    assert_eq!(shell.get_content_height(), height - TITLE_STRIP);
    assert_eq!(shell.get_marker_text(), "示例");
    assert!(!shell.get_strip_open());
    assert!(!shell.get_error_drawn(), "没有错误时不占一行");
    shell.set_error("保存失败：磁盘".into());
    slint::platform::update_timers_and_animations();
    assert!(shell.get_error_drawn());
    assert_eq!(shell.get_error_text(), "保存失败：磁盘");
    assert_eq!(shell.get_pin_x(), 320.0 - 56.0);
    assert_eq!(shell.get_content_y(), TITLE_STRIP);
    assert_eq!(shell.get_content_height(), height - TITLE_STRIP - 24.0);
    shell.set_error("".into());
    slint::platform::update_timers_and_animations();
    assert!(!shell.get_error_drawn());
    assert_eq!(shell.get_content_height(), height - TITLE_STRIP);

    shell.set_show_source(true);
    shell.set_source("便签 · 甲".into());
    slint::platform::update_timers_and_animations();
    assert!(shell.get_source_drawn(), "打开来源行才画");
    assert_eq!(shell.get_source_y(), TITLE_STRIP);
    assert_eq!(shell.get_source_h(), SOURCE_ROW);
    assert_eq!(shell.get_content_y(), TITLE_STRIP + SOURCE_ROW);
    shell.set_show_pin(false);
    slint::platform::update_timers_and_animations();
    assert!(!shell.get_pin_drawn(), "关掉钉住后按钮不在");
    assert_eq!(shell.get_close_x(), 320.0 - 28.0, "关闭仍在右上");
    shell.set_show_pin(true);
    shell.set_show_source(false);
    shell.set_source("".into());
    slint::platform::update_timers_and_animations();
    assert!(shell.get_pin_drawn());
    assert!(!shell.get_source_drawn(), "工具默认不画来源行");
    assert_eq!(shell.get_content_y(), TITLE_STRIP);
    assert_eq!(shell.get_pin_x(), 320.0 - 56.0);

    let pins = Rc::new(Cell::new(0));
    let closes = Rc::new(Cell::new(0));
    let drags = Rc::new(Cell::new(0));
    shell.on_pin_clicked({
        let pins = Rc::clone(&pins);
        move || pins.set(pins.get() + 1)
    });
    shell.on_close_clicked({
        let closes = Rc::clone(&closes);
        move || closes.set(closes.get() + 1)
    });
    shell.on_drag_requested({
        let drags = Rc::clone(&drags);
        move || drags.set(drags.get() + 1)
    });
    click(&shell, 16.0, 14.0);
    click(&shell, shell.get_pin_x() + 14.0, 14.0);
    click(&shell, shell.get_close_x() + 14.0, 14.0);
    assert_eq!((drags.get(), pins.get(), closes.get()), (1, 1, 1));

    shell.set_pinned(false);
    slint::platform::update_timers_and_animations();
    assert!(!shell.get_above_ordinary(), "钉住关掉后离开最上层");

    shell.set_dock_edge("top".into());
    slint::platform::update_timers_and_animations();
    assert!(shell.get_strip_open());
    assert!(
        shell.get_strip_x() + shell.get_strip_width() <= shell.get_pin_x(),
        "上沿把手不盖住钉住"
    );

    shell.set_dock_edge("left".into());
    slint::platform::update_timers_and_animations();
    assert_eq!(shell.get_strip_width(), logical(HANDLE_THICKNESS_LOGICAL));
    assert_eq!(shell.get_strip_height(), logical(HANDLE_LENGTH_LOGICAL));
    let strips = Rc::new(Cell::new(0));
    shell.on_strip_clicked({
        let strips = Rc::clone(&strips);
        move || strips.set(strips.get() + 1)
    });
    click(
        &shell,
        shell.get_strip_x() + shell.get_strip_width() / 2.0,
        shell.get_strip_y() + shell.get_strip_height() / 2.0,
    );
    assert_eq!(strips.get(), 1, "点展开时的把手");

    shell.set_thin(true);
    shell.set_collapsed(true);
    shell.window().set_size(slint::PhysicalSize::new(
        HANDLE_THICKNESS_LOGICAL,
        HANDLE_LENGTH_LOGICAL,
    ));
    slint::platform::update_timers_and_animations();
    assert!(!shell.get_content_open());
    assert!(!shell.get_pin_drawn());
    assert!(!shell.get_close_drawn());
    assert!(shell.get_collapsed_handle_open());
    assert_eq!(shell.get_drag_band(), 0.0);

    let collapsed_drags = Rc::new(Cell::new(0));
    shell.on_drag_requested({
        let collapsed_drags = Rc::clone(&collapsed_drags);
        move || collapsed_drags.set(collapsed_drags.get() + 1)
    });
    click(
        &shell,
        logical(HANDLE_THICKNESS_LOGICAL) / 2.0,
        logical(HANDLE_LENGTH_LOGICAL) / 2.0,
    );
    assert_eq!(collapsed_drags.get(), 1, "收起的把手按下就拖");

    let source = include_str!("../ui/float_shell.slint");
    assert!(source.contains("resize-border-width: thin ? 0px : 6px;"));
    assert!(source.contains("min-width: thin ? 1px : min(200px, limit-width);"));
    assert!(source.contains("min-height: thin ? 1px : min(120px, limit-height);"));
    assert!(source.contains("show-source: false"));
    assert!(source.contains("source-row.visible"));

    let _probe = FloatShellProbe::new().expect("内容槽能放进子元素");
}

#[test]
fn reloading_rows_keeps_the_window_position_and_size() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    let _ = slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))));
    let shell = FloatShell::new().expect("tool shell");
    // 软件渲染的窗口适配器不保存屏幕坐标，set_position 读回来仍是默认值。
    // 尺寸会留下。重载只换标题和行，这两项都必须与换之前相同。
    shell.window().set_size(slint::PhysicalSize::new(280, 240));
    shell.show().expect("show");
    slint::platform::update_timers_and_animations();
    let before = shell.window().position();
    let before_size = shell.window().size();
    assert_eq!((before_size.width, before_size.height), (280, 240));

    shell.set_heading("API 额度".into());
    shell.set_rows(ModelRc::new(VecModel::from(vec![crate::ToolRow {
        kind: "text".into(),
        id: "summary".into(),
        caption: "已用 20 / 100".into(),
        label: "".into(),
        progress: 0.2,
    }])));
    shell.set_error("".into());
    slint::platform::update_timers_and_animations();

    let after = shell.window().position();
    let after_size = shell.window().size();
    assert_eq!(
        (after.x, after.y),
        (before.x, before.y),
        "重载内容不移动窗口"
    );
    assert_eq!(
        (after_size.width, after_size.height),
        (before_size.width, before_size.height),
        "重载内容不改变窗口大小"
    );
}
