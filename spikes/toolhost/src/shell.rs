//! 验证用的悬浮窗外壳。软件渲染可以在没有桌面的测试里画出组件。
//! Windows 11 桌面上这扇窗口看起来怎么样，不在自动测试里，记录标未测。

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Mutex;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::host::{Session, WidgetViewKind};

slint::include_modules!();

static PLATFORM_LOCK: Mutex<()> = Mutex::new(());

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn shared_window() -> Rc<MinimalSoftwareWindow> {
    thread_local! {
        static WINDOW: RefCell<Option<Rc<MinimalSoftwareWindow>>> = const { RefCell::new(None) };
    }
    WINDOW.with(|slot| {
        if let Some(window) = slot.borrow().clone() {
            return window;
        }
        let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
        slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))))
            .expect("软件渲染平台只能装一次");
        *slot.borrow_mut() = Some(Rc::clone(&window));
        window
    })
}

pub fn platform_lock() -> std::sync::MutexGuard<'static, ()> {
    PLATFORM_LOCK.lock().expect("外壳锁")
}

fn wire_clicks(ui: &ToolShell, session: &Rc<Session>) {
    let session_click = Rc::clone(session);
    let ui_click = ui.as_weak();
    ui.on_row_clicked(move |id| {
        session_click.click(id.as_str());
        if let Some(ui) = ui_click.upgrade() {
            refresh(&ui, &session_click);
        }
    });
}

pub fn refresh(ui: &ToolShell, session: &Session) {
    let snap = session.snapshot();
    let rows: Vec<Row> = snap
        .widgets
        .iter()
        .map(|widget| Row {
            kind: kind_name(widget.kind).into(),
            id: widget.id.clone().into(),
            caption: widget.caption.clone().into(),
            label: widget.label.clone().into(),
            progress: widget.progress as f32,
        })
        .collect();
    ui.set_rows(ModelRc::new(VecModel::from(rows)));
    ui.set_tool_name(snap.name.into());
    ui.set_status_text(snap.status.into());
}

pub fn run_window(session: Rc<Session>) -> Result<(), slint::PlatformError> {
    let _ = slint::BackendSelector::new()
        .renderer_name("software".to_string())
        .select();
    session.on_load();
    let ui = ToolShell::new()?;
    refresh(&ui, &session);
    wire_clicks(&ui, &session);
    let ui_drag = ui.as_weak();
    ui.on_drag_requested(move || {
        let Some(ui) = ui_drag.upgrade() else {
            return;
        };
        let _ = ui.window().with_winit_window(|window| {
            let _ = window.drag_window();
        });
    });
    let session_tick = Rc::clone(&session);
    let ui_tick = ui.as_weak();
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(1),
        move || {
            session_tick.advance(1.0);
            if let Some(ui) = ui_tick.upgrade() {
                refresh(&ui, &session_tick);
            }
        },
    );
    ui.run()?;
    session.on_unload();
    Ok(())
}

pub fn attach_for_test(session: Rc<Session>) -> ToolShell {
    let window = shared_window();
    window.set_size(slint::PhysicalSize::new(320, 360));
    let ui = ToolShell::new().expect("验证窗口");
    refresh(&ui, &session);
    wire_clicks(&ui, &session);
    ui.show().expect("显示验证窗口");
    slint::platform::update_timers_and_animations();
    ui
}

fn kind_name(kind: WidgetViewKind) -> &'static str {
    match kind {
        WidgetViewKind::Text => "text",
        WidgetViewKind::Button => "button",
        WidgetViewKind::Timer => "timer",
        WidgetViewKind::Progress => "progress",
        WidgetViewKind::List => "list",
        WidgetViewKind::Input => "input",
        WidgetViewKind::Toggle => "toggle",
    }
}

pub fn click(ui: &ToolShell, x: f32, y: f32) {
    use slint::platform::{PointerEventButton, WindowEvent};
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
    slint::platform::update_timers_and_animations();
}

pub fn green_pixels(ui: &ToolShell) -> usize {
    use slint::platform::software_renderer::Rgb565Pixel;
    let _ = ui.get_row_count();
    let window = shared_window();
    let mut buffer = vec![Rgb565Pixel(0); 320 * 360];
    window.request_redraw();
    let drawn = window.draw_if_needed(|renderer| {
        renderer.render(&mut buffer, 320);
    });
    assert!(drawn, "外壳画出了一帧");
    buffer
        .into_iter()
        .filter(|pixel| {
            let red = (pixel.0 >> 11) & 0x1f;
            let green = (pixel.0 >> 5) & 0x3f;
            let blue = pixel.0 & 0x1f;
            green > 20 && green > red.saturating_mul(2) && green > blue.saturating_mul(2)
        })
        .count()
}
