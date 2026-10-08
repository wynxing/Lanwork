//! 背景用亚克力还是纯色。纯函数，不读系统。
//!
//! `DWMWA_SYSTEMBACKDROP_TYPE` 的最低客户端是 Windows 11 Build 22621。
//! 这是该 API 的能力边界，不是产品最低版本。产品最低 build 仍等 #9 第 19 项。

pub const MIN_BACKDROP_BUILD: u32 = 22621;

/// 浅色纯色。产品规格只要求与主题一致，没有写死色值。
/// 这里用 Windows 11 窗口底色，供这次测量对照截图像素。
pub const LIGHT_SOLID: Rgb = Rgb {
    r: 0xF3,
    g: 0xF3,
    b: 0xF3,
};

/// 暗色纯色，对应 Windows 11 暗色窗口底色。
pub const DARK_SOLID: Rgb = Rgb {
    r: 0x20,
    g: 0x20,
    b: 0x20,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackdropInput {
    pub build: u32,
    pub transparency_enabled: bool,
    pub battery_saver: bool,
    pub render_failed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackdropDecision {
    pub use_acrylic: bool,
    pub api_unavailable: bool,
    pub transparency_off: bool,
    pub battery_saver: bool,
    pub render_failed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderFailureRead {
    NotFailed,
    Failed,
    Inconclusive,
}

pub fn solid_rgb(dark: bool) -> Rgb {
    if dark { DARK_SOLID } else { LIGHT_SOLID }
}

pub fn choose_backdrop(input: BackdropInput) -> BackdropDecision {
    let api_unavailable = input.build < MIN_BACKDROP_BUILD;
    BackdropDecision {
        use_acrylic: !api_unavailable
            && input.transparency_enabled
            && !input.battery_saver
            && !input.render_failed,
        api_unavailable,
        transparency_off: !input.transparency_enabled,
        battery_saver: input.battery_saver,
        render_failed: input.render_failed,
    }
}

pub fn pixel_is_opaque_black(rgb: Rgb) -> bool {
    rgb.r <= 8 && rgb.g <= 8 && rgb.b <= 8
}

pub fn pixel_is_bright(rgb: Rgb) -> bool {
    u16::from(rgb.r) + u16::from(rgb.g) + u16::from(rgb.b) > 40
}

/// 空面板里声明为透明的区域。快照 alpha 很高说明渲染器没有留出透明像素。
pub fn framebuffer_transparency_failed(pixel: Rgba) -> bool {
    pixel.a >= 250
}

/// 屏幕上的像素。窗口外不亮时，纯黑可能是壁纸，不能当成渲染失败。
pub fn render_failure_from_screen(inside: &[Rgb], outside: Rgb) -> RenderFailureRead {
    if inside.is_empty() {
        return RenderFailureRead::Inconclusive;
    }
    if !inside.iter().copied().all(pixel_is_opaque_black) {
        return RenderFailureRead::NotFailed;
    }
    if pixel_is_bright(outside) {
        RenderFailureRead::Failed
    } else {
        RenderFailureRead::Inconclusive
    }
}

pub fn pixel_near(rgb: Rgb, target: Rgb, tolerance: u8) -> bool {
    rgb.r.abs_diff(target.r) <= tolerance
        && rgb.g.abs_diff(target.g) <= tolerance
        && rgb.b.abs_diff(target.b) <= tolerance
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_input() -> BackdropInput {
        BackdropInput {
            build: MIN_BACKDROP_BUILD,
            transparency_enabled: true,
            battery_saver: false,
            render_failed: false,
        }
    }

    #[test]
    fn acrylic_when_build_and_system_allow_it() {
        let decision = choose_backdrop(ok_input());
        assert!(decision.use_acrylic);
        assert!(!decision.api_unavailable);
    }

    #[test]
    fn build_22620_does_not_apply_backdrop() {
        let decision = choose_backdrop(BackdropInput {
            build: MIN_BACKDROP_BUILD - 1,
            ..ok_input()
        });
        assert!(!decision.use_acrylic);
        assert!(decision.api_unavailable);
    }

    #[test]
    fn transparency_off_battery_saver_and_render_failure_are_solid() {
        let off = choose_backdrop(BackdropInput {
            transparency_enabled: false,
            ..ok_input()
        });
        assert!(!off.use_acrylic);
        assert!(off.transparency_off);

        let battery = choose_backdrop(BackdropInput {
            battery_saver: true,
            ..ok_input()
        });
        assert!(!battery.use_acrylic);
        assert!(battery.battery_saver);

        let failed = choose_backdrop(BackdropInput {
            render_failed: true,
            ..ok_input()
        });
        assert!(!failed.use_acrylic);
        assert!(failed.render_failed);
    }

    #[test]
    fn screen_black_needs_a_bright_outside_pixel() {
        let black = Rgb { r: 0, g: 0, b: 0 };
        let bright = Rgb {
            r: 240,
            g: 240,
            b: 240,
        };
        assert_eq!(
            render_failure_from_screen(&[black, black], bright),
            RenderFailureRead::Failed
        );
        assert_eq!(
            render_failure_from_screen(&[black], black),
            RenderFailureRead::Inconclusive
        );
        assert_eq!(
            render_failure_from_screen(&[bright], black),
            RenderFailureRead::NotFailed
        );
    }

    #[test]
    fn opaque_framebuffer_pixel_is_a_transparency_failure() {
        assert!(framebuffer_transparency_failed(Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 255,
        }));
        assert!(!framebuffer_transparency_failed(Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        }));
    }

    #[test]
    fn solid_colors_match_the_theme_flag() {
        assert_eq!(solid_rgb(false), LIGHT_SOLID);
        assert_eq!(solid_rgb(true), DARK_SOLID);
        assert!(pixel_near(
            Rgb {
                r: 0xF0,
                g: 0xF4,
                b: 0xF2,
            },
            LIGHT_SOLID,
            8
        ));
    }
}
