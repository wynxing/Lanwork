//! 应用页的静态预览。不是产品界面。
//!
//! 用软件渲染画出几个状态，写成 PNG。不安装工具，不读数据目录。

use std::path::PathBuf;
use std::rc::Rc;

use slint::ComponentHandle;
use slint::platform::software_renderer::{
    MinimalSoftwareWindow, PremultipliedRgbaColor, RepaintBufferType, TargetPixel,
};
use slint::platform::{Platform, WindowAdapter, WindowEvent};

slint::include_modules!();

struct Headless(Rc<MinimalSoftwareWindow>);

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

#[derive(Clone, Copy)]
struct Rgb {
    r: u8,
    g: u8,
    b: u8,
}

impl TargetPixel for Rgb {
    fn blend(&mut self, color: PremultipliedRgbaColor) {
        let alpha = u16::from(color.alpha);
        if alpha == 0 {
            return;
        }
        if alpha == 255 {
            self.r = color.red;
            self.g = color.green;
            self.b = color.blue;
            return;
        }
        let keep = 255 - alpha;
        self.r = (u16::from(color.red) + u16::from(self.r) * keep / 255) as u8;
        self.g = (u16::from(color.green) + u16::from(self.g) * keep / 255) as u8;
        self.b = (u16::from(color.blue) + u16::from(self.b) * keep / 255) as u8;
    }

    fn from_rgb(red: u8, green: u8, blue: u8) -> Self {
        Self {
            r: red,
            g: green,
            b: blue,
        }
    }
}

struct Scene {
    name: &'static str,
    file: &'static str,
    width: u32,
    height: u32,
}

const SCENES: &[Scene] = &[
    Scene {
        name: "list",
        file: "list.png",
        width: 760,
        height: 560,
    },
    Scene {
        name: "empty",
        file: "empty.png",
        width: 760,
        height: 560,
    },
    Scene {
        name: "menu",
        file: "menu.png",
        width: 760,
        height: 560,
    },
    Scene {
        name: "import",
        file: "import-confirm.png",
        width: 760,
        height: 560,
    },
    Scene {
        name: "uninstall",
        file: "uninstall-confirm.png",
        width: 760,
        height: 560,
    },
    Scene {
        name: "error",
        file: "script-timeout.png",
        width: 320,
        height: 168,
    },
];

fn main() {
    let mut args = std::env::args().skip(1);
    let mut out = PathBuf::from("/opt/cursor/artifacts/apps-page-preview");
    while let Some(arg) = args.next() {
        if arg == "--out" {
            out = PathBuf::from(args.next().unwrap_or_else(|| usage()));
        } else if arg == "--help" || arg == "-h" {
            usage();
        } else {
            eprintln!("lanwork-apps-preview: 不认识的参数 {arg}");
            usage();
        }
    }
    if let Err(err) = render_all(&out) {
        eprintln!("lanwork-apps-preview: {err}");
        std::process::exit(1);
    }
}

fn usage() -> ! {
    eprintln!("用法: lanwork-apps-preview --out <目录>");
    std::process::exit(2);
}

fn render_all(dir: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|err| format!("创建目录失败：{err}"))?;
    // 每次都换一块新缓冲，必须整帧重画。复用缓冲只会补脏区域，标题会空掉。
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(Headless(Rc::clone(&window))))
        .map_err(|err| format!("软件渲染平台只能装一次：{err}"))?;
    let ui = AppsPreview::new().map_err(|err| format!("创建预览失败：{err}"))?;
    for scene in SCENES {
        render_scene(&ui, &window, dir, scene)?;
        println!("{}", dir.join(scene.file).display());
    }
    Ok(())
}

fn render_scene(
    ui: &AppsPreview,
    window: &MinimalSoftwareWindow,
    dir: &std::path::Path,
    scene: &Scene,
) -> Result<(), String> {
    // 两倍逻辑像素，文字在截图里更清楚。版式仍按 760×560 和 320×168。
    let scale = 2u32;
    let physical_w = scene.width * scale;
    let physical_h = scene.height * scale;
    window.set_size(slint::PhysicalSize::new(physical_w, physical_h));
    ui.window().dispatch_event(WindowEvent::ScaleFactorChanged {
        scale_factor: scale as f32,
    });
    ui.set_scene(scene.name.into());
    ui.set_frame_width(scene.width as f32);
    ui.set_frame_height(scene.height as f32);
    ui.show().map_err(|err| format!("显示预览失败：{err}"))?;
    slint::platform::update_timers_and_animations();
    slint::platform::update_timers_and_animations();
    window.request_redraw();
    let mut pixels = vec![
        Rgb {
            r: 0xf3,
            g: 0xf3,
            b: 0xf3
        };
        (physical_w * physical_h) as usize
    ];
    let drawn = window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, physical_w as usize);
    });
    if !drawn {
        return Err(format!("{} 没有画出一帧", scene.file));
    }
    write_png(&dir.join(scene.file), physical_w, physical_h, &pixels)
}

fn write_png(
    path: &std::path::Path,
    width: u32,
    height: u32,
    pixels: &[Rgb],
) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|err| format!("创建截图失败：{err}"))?;
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder
        .write_header()
        .map_err(|err| format!("PNG 头失败：{err}"))?;
    let mut rgb = Vec::with_capacity(pixels.len() * 3);
    for pixel in pixels {
        rgb.push(pixel.r);
        rgb.push(pixel.g);
        rgb.push(pixel.b);
    }
    writer
        .write_image_data(&rgb)
        .map_err(|err| format!("PNG 数据失败：{err}"))?;
    Ok(())
}
