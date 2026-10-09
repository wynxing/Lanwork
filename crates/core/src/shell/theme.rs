//! 主题解析。
//!
//! 明和暗按配置。跟随系统时用调用方读到的 `AppsUseLightTheme`。
//! 读不到系统值时不猜测，返回 [`ResolvedTheme::Unchanged`]。

use crate::config::Theme;

/// 注册表里的系统应用主题。缺失或无法读取时是 [`SystemLight::Unknown`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemLight {
    Light,
    Dark,
    Unknown,
}

/// 要写进界面调色板的结果。`Unchanged` 表示这次不要改。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedTheme {
    Light,
    Dark,
    Unchanged,
}

impl ResolvedTheme {
    #[must_use]
    pub fn is_dark(self) -> Option<bool> {
        match self {
            Self::Dark => Some(true),
            Self::Light => Some(false),
            Self::Unchanged => None,
        }
    }
}

/// `apps_use_light_theme` 的 DWORD：0 是暗，其他非空值是明。
#[must_use]
pub fn system_light_from_dword(value: Option<u32>) -> SystemLight {
    match value {
        None => SystemLight::Unknown,
        Some(0) => SystemLight::Dark,
        Some(_) => SystemLight::Light,
    }
}

#[must_use]
pub fn resolve_theme(theme: Theme, system: SystemLight) -> ResolvedTheme {
    match theme {
        Theme::Light => ResolvedTheme::Light,
        Theme::Dark => ResolvedTheme::Dark,
        Theme::System => match system {
            SystemLight::Light => ResolvedTheme::Light,
            SystemLight::Dark => ResolvedTheme::Dark,
            SystemLight::Unknown => ResolvedTheme::Unchanged,
        },
    }
}

/// `WM_SETTINGCHANGE` 的 `lParam` 是不是沉浸式颜色集。
///
/// 空指针不是。比较时忽略 ASCII 大小写。
#[must_use]
pub fn is_immersive_color_set(lparam: Option<&str>) -> bool {
    lparam.is_some_and(|text| text.eq_ignore_ascii_case("ImmersiveColorSet"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_theme_ignores_the_system_value() {
        assert_eq!(
            resolve_theme(Theme::Light, SystemLight::Dark),
            ResolvedTheme::Light
        );
        assert_eq!(
            resolve_theme(Theme::Dark, SystemLight::Light),
            ResolvedTheme::Dark
        );
    }

    #[test]
    fn system_theme_follows_the_dword_and_unknown_does_not_guess() {
        assert_eq!(system_light_from_dword(Some(0)), SystemLight::Dark);
        assert_eq!(system_light_from_dword(Some(1)), SystemLight::Light);
        assert_eq!(system_light_from_dword(None), SystemLight::Unknown);
        assert_eq!(
            resolve_theme(Theme::System, SystemLight::Unknown),
            ResolvedTheme::Unchanged
        );
        assert_eq!(
            resolve_theme(Theme::System, SystemLight::Dark).is_dark(),
            Some(true)
        );
        assert!(is_immersive_color_set(Some("immersivecolorset")));
        assert!(!is_immersive_color_set(None));
        assert!(!is_immersive_color_set(Some("ImmersiveColorSetExtra")));
    }
}
