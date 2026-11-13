//! 搜索条右侧应用墙入口。图标固定在热区中央，名称是按钮左侧的短提示。

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use slint::ComponentHandle;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType,
};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};

use crate::SearchBar;

thread_local! {
    static FRAME: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
}

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn open() -> SearchBar {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(680, 56));
    if slint::platform::set_platform(Box::new(Headless(Rc::clone(&window)))).is_ok() {
        FRAME.with(|slot| *slot.borrow_mut() = Some(Rc::clone(&window)));
    }
    let ui = SearchBar::new().expect("search bar");
    ui.window().set_size(slint::PhysicalSize::new(680, 56));
    ui.show().expect("show");
    slint::platform::update_timers_and_animations();
    ui
}

fn pointer(ui: &SearchBar, x: f32, y: f32) {
    ui.window().dispatch_event(WindowEvent::PointerMoved {
        position: slint::LogicalPosition::new(x, y),
    });
}

fn click(ui: &SearchBar, x: f32, y: f32) {
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

fn hits(ui: &SearchBar) -> Rc<Cell<i32>> {
    let hits = Rc::new(Cell::new(0));
    let record = Rc::clone(&hits);
    ui.on_toggle_wall(move || record.set(record.get() + 1));
    hits
}

fn icon_center(ui: &SearchBar) -> (f32, f32) {
    (
        ui.get_wall_icon_x() + ui.get_wall_icon_size() / 2.0,
        ui.get_wall_icon_y() + ui.get_wall_icon_size() / 2.0,
    )
}

fn assert_icon_stays(ui: &SearchBar, at: (f32, f32, f32)) {
    let x = ui.get_wall_icon_x();
    let y = ui.get_wall_icon_y();
    let size = ui.get_wall_icon_size();
    let expected_x = ui.get_wall_hit_x() + (ui.get_wall_hit_w() - size) / 2.0;
    let expected_y = ui.get_wall_hit_y() + (ui.get_wall_hit_h() - size) / 2.0;
    assert!((size - 18.0).abs() < 0.5, "图标保持 18px，实际 {size}");
    assert!(
        (x - expected_x).abs() < 0.5 && (y - expected_y).abs() < 0.5,
        "图标在热区中央 {x},{y} vs {expected_x},{expected_y}"
    );
    assert!(
        (x - at.0).abs() < 0.5 && (y - at.1).abs() < 0.5 && (size - at.2).abs() < 0.5,
        "图标相对静止时没有位移 {x},{y},{size} vs {},{},{}",
        at.0,
        at.1,
        at.2
    );
}

fn assert_tip_left_of_button(ui: &SearchBar) {
    let x = ui.get_wall_tip_x();
    let y = ui.get_wall_tip_y();
    let w = ui.get_wall_tip_w();
    let h = ui.get_wall_tip_h();
    assert!(w > 8.0 && h > 8.0, "提示有尺寸 {w}x{h}");
    assert!(x >= -0.5, "提示左缘在窗口内 {x}");
    assert!(y >= -0.5, "提示上缘在窗口内 {y}");
    assert!(x + w <= 680.5, "提示右缘在窗口内 {x}+{w}");
    assert!(y + h <= 56.5, "提示下缘在窗口内 {y}+{h}");
    let button = ui.get_wall_hit_x();
    assert!(
        x + w <= button - ui.get_wall_tip_gap() + 0.5,
        "提示在按钮左侧并留出间距 {x}+{w} vs {button}-{}",
        ui.get_wall_tip_gap()
    );
}

fn wait_tip(ui: &SearchBar) {
    let delay = u64::try_from(ui.get_wall_tip_delay().max(0)).unwrap_or(400);
    std::thread::sleep(std::time::Duration::from_millis(delay + 80));
    slint::platform::update_timers_and_animations();
}

fn press(ui: &SearchBar, text: slint::SharedString) {
    ui.window().dispatch_event(WindowEvent::KeyPressed { text });
}

#[test]
fn wall_entry_is_a_stable_icon_and_the_name_is_a_tip() {
    let ui = open();
    assert!(ui.get_wall_hit_w() >= 40.0, "热区够宽");
    assert!(ui.get_wall_hit_h() >= 40.0, "热区够高");
    assert_eq!(ui.get_shown_wall_name().as_str(), "", "静止时不显示名称");
    let icons = (
        ui.get_wall_icon_x(),
        ui.get_wall_icon_y(),
        ui.get_wall_icon_size(),
    );
    assert_icon_stays(&ui, icons);

    let toggles = hits(&ui);
    // 这两点以前落在右侧 96×56 的槽里、图标和文字之外，按下就会打开应用墙。
    click(&ui, 590.0, 28.0);
    click(&ui, 632.0, 4.0);
    assert_eq!(
        toggles.get(),
        0,
        "图标左侧和按钮上方的搜索条底色不打开应用墙"
    );

    let (x, y) = icon_center(&ui);
    click(&ui, x, y);
    assert_eq!(toggles.get(), 1, "点图标打开应用墙");
    click(&ui, ui.get_wall_hit_x() + 2.0, y);
    click(&ui, ui.get_wall_hit_x() + ui.get_wall_hit_w() - 2.0, y);
    assert_eq!(toggles.get(), 3, "热区边缘同样打开应用墙");
    click(&ui, 400.0, 28.0);
    assert_eq!(toggles.get(), 3, "输入框不打开应用墙");

    ui.set_text("一二三四五六七八九十".repeat(12).into());
    slint::platform::update_timers_and_animations();
    click(&ui, x, y);
    assert_eq!(toggles.get(), 4, "长文本不盖住图标");
    assert_icon_stays(&ui, icons);

    pointer(&ui, 200.0, 28.0);
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_wall_name().as_str(), "");

    pointer(&ui, x, y);
    slint::platform::update_timers_and_animations();
    assert_eq!(
        ui.get_shown_wall_name().as_str(),
        "",
        "刚悬停时名称还不出现"
    );
    assert_icon_stays(&ui, icons);
    pointer(&ui, 200.0, 28.0);
    wait_tip(&ui);
    assert_eq!(
        ui.get_shown_wall_name().as_str(),
        "",
        "延迟结束前移开，名称不出现"
    );

    pointer(&ui, x, y);
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_wall_name().as_str(), "");
    wait_tip(&ui);
    assert_eq!(ui.get_shown_wall_name().as_str(), "应用墙");
    assert_icon_stays(&ui, icons);
    assert_tip_left_of_button(&ui);
    let tip_x = ui.get_wall_tip_x() + ui.get_wall_tip_w() / 2.0;
    let tip_y = ui.get_wall_tip_y() + ui.get_wall_tip_h() / 2.0;
    click(&ui, tip_x, tip_y);
    assert_eq!(toggles.get(), 4, "提示不接收点击，也不打开应用墙");
    pointer(&ui, 200.0, 28.0);
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_wall_name().as_str(), "", "移出后名称消失");
    assert_icon_stays(&ui, icons);

    ui.invoke_focus_input();
    slint::platform::update_timers_and_animations();
    press(&ui, slint::platform::Key::Tab.into());
    slint::platform::update_timers_and_animations();
    assert_eq!(
        ui.get_shown_wall_name().as_str(),
        "应用墙",
        "Tab 从输入框进入按钮后名称立刻出现"
    );
    assert_icon_stays(&ui, icons);
    assert_tip_left_of_button(&ui);
    press(&ui, " ".into());
    assert_eq!(toggles.get(), 5, "空格打开应用墙");
    press(&ui, slint::platform::Key::Return.into());
    assert_eq!(toggles.get(), 6, "回车打开应用墙");

    let escapes = Rc::new(Cell::new(0));
    {
        let escapes = Rc::clone(&escapes);
        ui.on_escape(move || escapes.set(escapes.get() + 1));
    }
    press(&ui, slint::platform::Key::Escape.into());
    assert_eq!(escapes.get(), 1, "按钮上的 Esc 交给搜索条");
    assert_eq!(toggles.get(), 6, "Esc 不打开应用墙");

    ui.invoke_focus_input();
    slint::platform::update_timers_and_animations();
    assert_eq!(
        ui.get_shown_wall_name().as_str(),
        "",
        "焦点回到输入框后名称消失"
    );
    assert_icon_stays(&ui, icons);

    ui.set_wall_open(true);
    slint::platform::update_timers_and_animations();
    assert_icon_stays(&ui, icons);
    pointer(&ui, x, y);
    slint::platform::update_timers_and_animations();
    wait_tip(&ui);
    assert_eq!(ui.get_shown_wall_name().as_str(), "应用墙");
    assert_icon_stays(&ui, icons);

    let source = include_str!("../ui/searchbar.slint");
    let button = source.split("wall-touch := TouchArea").nth(1).unwrap();
    let button = button.split("clicked =>").next().unwrap();
    assert!(
        button.contains("accessible-label: \"应用墙\""),
        "可访问名称还在"
    );
    assert!(!button.contains("text:"), "名称不写在按钮里");
}

fn paint(path: &str) {
    const WIDTH: usize = 680;
    const HEIGHT: usize = 56;
    FRAME.with(|slot| {
        let window = slot.borrow().clone().expect("software window");
        window.request_redraw();
        let rendered = window.draw_if_needed(|renderer| {
            let mut buffer = vec![PremultipliedRgbaColor::default(); WIDTH * HEIGHT];
            renderer.render(&mut buffer, WIDTH);
            let mut file = std::fs::File::create(path).expect("ppm");
            use std::io::Write;
            write!(file, "P6\n{WIDTH} {HEIGHT}\n255\n").expect("header");
            for pixel in buffer {
                file.write_all(&[pixel.red, pixel.green, pixel.blue])
                    .expect("pixel");
            }
        });
        assert!(rendered, "这一帧要画出来");
    });
}

/// 入口静止和悬停的源画面。默认不跑，避免测试写文件。
/// `LANWORK_WALL_SHOTS=/tmp/wall cargo test --bin lanwork wall_entry_shots -- --ignored --exact`
#[test]
#[ignore]
fn wall_entry_shots() {
    let dir = std::env::var("LANWORK_WALL_SHOTS").unwrap_or_else(|_| "/tmp/wall-shots".into());
    std::fs::create_dir_all(&dir).expect("shot dir");
    let ui = open();
    ui.set_dark(false);
    ui.set_text("周报".into());
    slint::platform::update_timers_and_animations();
    pointer(&ui, 200.0, 28.0);
    ui.invoke_focus_input();
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_wall_name().as_str(), "");
    paint(&format!("{dir}/idle.ppm"));

    let (x, y) = icon_center(&ui);
    pointer(&ui, x, y);
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_wall_name().as_str(), "");
    wait_tip(&ui);
    assert_eq!(ui.get_shown_wall_name().as_str(), "应用墙");
    paint(&format!("{dir}/hover.ppm"));

    pointer(&ui, 200.0, 28.0);
    slint::platform::update_timers_and_animations();
    ui.invoke_focus_input();
    slint::platform::update_timers_and_animations();
    press(&ui, slint::platform::Key::Tab.into());
    slint::platform::update_timers_and_animations();
    assert_eq!(ui.get_shown_wall_name().as_str(), "应用墙");
    paint(&format!("{dir}/focus.ppm"));
}
