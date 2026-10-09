//! 卸载过滤、游戏链接、启动目录和两个应用动作的判断。
//!
//! 这些是实现选择，不是产品规则。可见行为以产品规格为准。
//! 过滤和游戏链接的调用方在 Windows 枚举里。非 Windows 构建只跑这里的单元测试。
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};

use super::model::{LaunchTarget, normalize_path_key};

/// Flow Launcher `HideUninstallersFilter` 的固定文件名。
const UNINSTALLER_FILE_NAMES: [&str; 4] = [
    "uninst.exe",
    "unins000.exe",
    "uninst000.exe",
    "uninstall.exe",
];

/// 同一拼写只留一次。Flow 的名单里有些词因语言不同而重复。
const UNINSTALLER_PREFIXES: [&str; 22] = [
    "uninstall",
    "卸载",
    "卸載",
    "видалити",
    "удалить",
    "désinstaller",
    "アンインストール",
    "deïnstalleren",
    "odinstaluj",
    "afinstallere",
    "deinstallieren",
    "삭제",
    "деинсталирај",
    "desinstalar",
    "disinstallare",
    "avinstallere",
    "odinštalovať",
    "kaldır",
    "odinstalovat",
    "إلغاء التثبيت",
    "gỡ bỏ",
    "הסרה",
];

const GAME_URL_PREFIXES: [&str; 3] = [
    "steam://run/",
    "steam://rungameid/",
    "com.epicgames.launcher://apps/",
];

/// 开始菜单、App Paths 和 PATH 上的卸载程序。商店应用和便携应用不调用这里。
#[must_use]
pub(crate) fn is_uninstaller(
    display_name: &str,
    target_file_name: &str,
    shortcut_file_name: &str,
) -> bool {
    let target = file_name_only(target_file_name);
    if UNINSTALLER_FILE_NAMES
        .iter()
        .any(|name| target.eq_ignore_ascii_case(name))
    {
        return true;
    }
    if ends_with_ignore_ascii(target, ".exe") && starts_with_prefix(target) {
        return true;
    }
    if starts_with_prefix(display_name.trim()) {
        return true;
    }
    let shortcut = file_name_only(shortcut_file_name);
    ends_with_ignore_ascii(shortcut, ".lnk") && starts_with_prefix(shortcut)
}

/// 前缀之后还有剩余时返回去掉首尾空白的原地址，否则 `None`。
#[must_use]
pub(crate) fn indexed_game_url(url: &str) -> Option<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    for prefix in GAME_URL_PREFIXES {
        if let Some(rest) = lower.strip_prefix(prefix)
            && !rest.is_empty()
        {
            return Some(trimmed.to_owned());
        }
    }
    None
}

/// 读 `.url` 正文。UTF-16 LE/BE 带 BOM 时按对应端序解码，其余按 UTF-8。
#[must_use]
pub(crate) fn internet_shortcut_url(bytes: &[u8]) -> Option<String> {
    let text = decode_shortcut_text(bytes);
    for line in text.lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("url") {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

/// 规范化路径与排除名单中的某一项相同。不看最后一段的名字。
#[must_use]
pub(crate) fn is_excluded_directory(dir: &Path, excluded: &[PathBuf]) -> bool {
    if excluded.is_empty() {
        return false;
    }
    let key = normalize_path_key(dir);
    if key.is_empty() {
        return false;
    }
    excluded.iter().any(|item| normalize_path_key(item) == key)
}

/// 有本地 exe 时可以提权。商店应用和协议链接不行，原因见架构文档。
#[must_use]
pub fn supports_run_as_admin(target: &LaunchTarget) -> bool {
    matches!(target, LaunchTarget::Path { .. })
}

/// 有本地 exe 且父目录非空。协议链接的这个动作待定，这里不提供。
#[must_use]
pub fn supports_open_containing_folder(target: &LaunchTarget) -> bool {
    containing_folder(target).is_some()
}

/// 目标 exe 的父目录。不是快捷方式所在的目录。
#[must_use]
pub fn containing_folder(target: &LaunchTarget) -> Option<PathBuf> {
    let LaunchTarget::Path { path, .. } = target else {
        return None;
    };
    let parent = path.parent()?;
    if parent.as_os_str().is_empty() {
        None
    } else {
        Some(parent.to_path_buf())
    }
}

fn file_name_only(text: &str) -> &str {
    let text = text.trim().trim_matches('"');
    text.rsplit(['\\', '/']).next().unwrap_or(text)
}

fn starts_with_prefix(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    let lowered = text.to_lowercase();
    UNINSTALLER_PREFIXES
        .iter()
        .any(|prefix| lowered.starts_with(&prefix.to_lowercase()))
}

fn ends_with_ignore_ascii(text: &str, suffix: &str) -> bool {
    if suffix.len() > text.len() {
        return false;
    }
    let start = text.len() - suffix.len();
    text.is_char_boundary(start) && text[start..].eq_ignore_ascii_case(suffix)
}

fn decode_shortcut_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        return utf16_units(&bytes[2..], true);
    }
    if bytes.starts_with(&[0xFE, 0xFF]) {
        return utf16_units(&bytes[2..], false);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn utf16_units(bytes: &[u8], little: bool) -> String {
    let units: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            if little {
                u16::from_le_bytes(*pair)
            } else {
                u16::from_be_bytes(*pair)
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_target(path: &str) -> LaunchTarget {
        LaunchTarget::Path {
            path: PathBuf::from(path),
            args: String::new(),
            working_directory: None,
        }
    }

    #[test]
    fn exact_uninstaller_file_names_are_filtered() {
        for name in [
            r"C:\App\unins000.exe",
            "uninst.exe",
            "UNINSTALL.EXE",
            "uninst000.exe",
        ] {
            assert!(is_uninstaller("工具", name, "工具.lnk"), "{name}");
        }
    }

    #[test]
    fn prefixed_exe_and_display_name_are_filtered() {
        assert!(is_uninstaller(
            "某程序",
            r"D:\App\Uninstall 某程序.exe",
            "某程序.lnk"
        ));
        assert!(is_uninstaller(
            "卸载 某程序",
            r"D:\App\helper.exe",
            "helper.lnk"
        ));
        assert!(is_uninstaller(
            "Remove helper",
            r"D:\App\helper.exe",
            "Uninstall helper.lnk"
        ));
        assert!(!is_uninstaller("微信", r"D:\WeChat\WeChat.exe", "微信.lnk"));
        assert!(!is_uninstaller(
            "My Uninstall",
            r"D:\App\helper.exe",
            "helper.lnk"
        ));
    }

    #[test]
    fn game_urls_keep_steam_and_epic_only() {
        assert_eq!(
            indexed_game_url("  steam://rungameid/570  ").as_deref(),
            Some("steam://rungameid/570")
        );
        assert!(indexed_game_url("STEAM://RUN/10").is_some());
        assert!(indexed_game_url("com.epicgames.launcher://apps/Foo").is_some());
        assert!(indexed_game_url("https://store.steampowered.com/app/570").is_none());
        assert!(indexed_game_url("http://example.com").is_none());
        assert!(indexed_game_url("steam://open/main").is_none());
        assert!(indexed_game_url("steam://run/").is_none());
        assert!(indexed_game_url("steam://rungameid").is_none());
        assert!(indexed_game_url("com.epicgames.launcher://store").is_none());
    }

    #[test]
    fn internet_shortcut_reads_the_first_url_line() {
        let text = "[InternetShortcut]\r\nIconFile=game.ico\r\nURL=steam://rungameid/570\r\n";
        assert_eq!(
            internet_shortcut_url(text.as_bytes()).as_deref(),
            Some("steam://rungameid/570")
        );
        let mut utf16 = vec![0xFF, 0xFE];
        for unit in "URL=steam://run/12\n".encode_utf16() {
            utf16.extend(unit.to_le_bytes());
        }
        assert_eq!(
            internet_shortcut_url(&utf16).as_deref(),
            Some("steam://run/12")
        );
        assert!(internet_shortcut_url(b"[InternetShortcut]\nIconFile=a.ico\n").is_none());
    }

    #[test]
    fn exclusion_uses_the_path_not_the_folder_name() {
        let startup = PathBuf::from(r"C:\ProgramData\Microsoft\Windows\Start Menu\Programs\启动");
        let excluded = [startup.clone()];
        assert!(is_excluded_directory(&startup, &excluded));
        assert!(is_excluded_directory(
            Path::new(r"c:/programdata/microsoft/windows/start menu/programs/启动"),
            &excluded
        ));
        assert!(!is_excluded_directory(
            Path::new(r"D:\Games\Startup"),
            &excluded
        ));
        assert!(!is_excluded_directory(
            Path::new(r"D:\Games\启动"),
            &excluded
        ));
        assert!(!is_excluded_directory(&startup, &[]));
    }

    #[test]
    fn admin_and_containing_folder_follow_the_target_kind() {
        let exe = path_target("C:/App/tool.exe");
        assert!(supports_run_as_admin(&exe));
        assert!(supports_open_containing_folder(&exe));
        assert_eq!(
            containing_folder(&exe).as_deref(),
            Some(Path::new("C:/App"))
        );
        assert!(containing_folder(&path_target("tool.exe")).is_none());
        let store = LaunchTarget::Aumid {
            aumid: "App".into(),
        };
        let url = LaunchTarget::Url {
            url: "steam://rungameid/1".into(),
        };
        assert!(!supports_run_as_admin(&store));
        assert!(!supports_open_containing_folder(&store));
        assert!(!supports_run_as_admin(&url));
        assert!(!supports_open_containing_folder(&url));
        assert!(containing_folder(&url).is_none());
    }
}
