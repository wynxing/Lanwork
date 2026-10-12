//! 热键字符串。
//!
//! 这是写入 `config.json` 的格式，也是设置页录入之后保存的格式。
//! 修饰键是 `Ctrl`、`Alt`、`Shift`、`Win`，再加一个主键。比较和保存前收成
//! `Ctrl+Alt+Shift+Win+键`。大小写不区分，`+` 两侧的空白会去掉。
//!
//! 主键是 `A`–`Z`、`0`–`9`、`F1`–`F24`，以及产品规格补上的空格、方向键、
//! 标点符号键、小键盘数字、小键盘运算符、Home、End、PageUp、PageDown、Insert、Delete。
//! 除 `F1`–`F24` 外必须至少有一个修饰键。`+` 用来分隔各段，所以小键盘加号写成
//! `NumpadAdd`，不写成 `+`。

use super::error::ConfigError;

/// 一个可以交给 `RegisterHotKey` 的热键。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Hotkey {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    pub key: HotkeyKey,
}

/// 热键里的主键。虚拟键码是 Win32 的 `VK_*` 值，调用方直接交给 `RegisterHotKey`。
///
/// `Numpad` 的数字是 `0`–`9`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotkeyKey {
    Letter(u8),
    Digit(u8),
    Function(u8),
    Space,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    Semicolon,
    Equals,
    Comma,
    Minus,
    Period,
    Slash,
    Backtick,
    LeftBracket,
    Backslash,
    RightBracket,
    Quote,
    Numpad(u8),
    NumpadMultiply,
    NumpadAdd,
    NumpadSubtract,
    NumpadDivide,
}

impl Hotkey {
    /// Win32 虚拟键码。字母和数字用 ASCII 大写，功能键从 `VK_F1`（0x70）起。
    #[must_use]
    pub fn virtual_key(self) -> u32 {
        self.key.virtual_key()
    }

    /// 固定顺序的字符串。修饰键顺序是 Ctrl、Alt、Shift、Win。
    #[must_use]
    pub fn canonical(self) -> String {
        let mut parts = Vec::new();
        if self.control {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.win {
            parts.push("Win");
        }
        let key = self.key.label();
        parts.push(key.as_str());
        parts.join("+")
    }

    fn has_modifier(self) -> bool {
        self.control || self.alt || self.shift || self.win
    }
}

impl HotkeyKey {
    /// 功能键可以单独使用。其余主键至少要有一个修饰键。
    #[must_use]
    pub(crate) fn requires_modifier(self) -> bool {
        !matches!(self, Self::Function(_))
    }

    /// 由 Win32 虚拟键码得到主键。不在键名表里时为 `None`。
    #[must_use]
    pub(crate) fn from_virtual_key(vk: u32) -> Option<Self> {
        Some(match vk {
            0x41..=0x5A => Self::Letter(vk as u8),
            0x30..=0x39 => Self::Digit(vk as u8),
            0x70..=0x87 => Self::Function((vk - 0x70 + 1) as u8),
            0x20 => Self::Space,
            0x26 => Self::Up,
            0x28 => Self::Down,
            0x25 => Self::Left,
            0x27 => Self::Right,
            0x24 => Self::Home,
            0x23 => Self::End,
            0x21 => Self::PageUp,
            0x22 => Self::PageDown,
            0x2D => Self::Insert,
            0x2E => Self::Delete,
            0xBA => Self::Semicolon,
            0xBB => Self::Equals,
            0xBC => Self::Comma,
            0xBD => Self::Minus,
            0xBE => Self::Period,
            0xBF => Self::Slash,
            0xC0 => Self::Backtick,
            0xDB => Self::LeftBracket,
            0xDC => Self::Backslash,
            0xDD => Self::RightBracket,
            0xDE => Self::Quote,
            0x60..=0x69 => Self::Numpad((vk - 0x60) as u8),
            0x6A => Self::NumpadMultiply,
            0x6B => Self::NumpadAdd,
            0x6D => Self::NumpadSubtract,
            0x6F => Self::NumpadDivide,
            _ => return None,
        })
    }

    fn virtual_key(self) -> u32 {
        match self {
            Self::Letter(letter) | Self::Digit(letter) => u32::from(letter),
            Self::Function(index) => 0x70 + u32::from(index - 1),
            Self::Space => 0x20,
            Self::Up => 0x26,
            Self::Down => 0x28,
            Self::Left => 0x25,
            Self::Right => 0x27,
            Self::Home => 0x24,
            Self::End => 0x23,
            Self::PageUp => 0x21,
            Self::PageDown => 0x22,
            Self::Insert => 0x2D,
            Self::Delete => 0x2E,
            Self::Semicolon => 0xBA,
            Self::Equals => 0xBB,
            Self::Comma => 0xBC,
            Self::Minus => 0xBD,
            Self::Period => 0xBE,
            Self::Slash => 0xBF,
            Self::Backtick => 0xC0,
            Self::LeftBracket => 0xDB,
            Self::Backslash => 0xDC,
            Self::RightBracket => 0xDD,
            Self::Quote => 0xDE,
            Self::Numpad(digit) => 0x60 + u32::from(digit),
            Self::NumpadMultiply => 0x6A,
            Self::NumpadAdd => 0x6B,
            Self::NumpadSubtract => 0x6D,
            Self::NumpadDivide => 0x6F,
        }
    }

    fn label(self) -> String {
        match self {
            Self::Letter(letter) | Self::Digit(letter) => char::from(letter).to_string(),
            Self::Function(index) => format!("F{index}"),
            Self::Space => "Space".to_owned(),
            Self::Up => "Up".to_owned(),
            Self::Down => "Down".to_owned(),
            Self::Left => "Left".to_owned(),
            Self::Right => "Right".to_owned(),
            Self::Home => "Home".to_owned(),
            Self::End => "End".to_owned(),
            Self::PageUp => "PageUp".to_owned(),
            Self::PageDown => "PageDown".to_owned(),
            Self::Insert => "Insert".to_owned(),
            Self::Delete => "Delete".to_owned(),
            Self::Semicolon => ";".to_owned(),
            Self::Equals => "=".to_owned(),
            Self::Comma => ",".to_owned(),
            Self::Minus => "-".to_owned(),
            Self::Period => ".".to_owned(),
            Self::Slash => "/".to_owned(),
            Self::Backtick => "`".to_owned(),
            Self::LeftBracket => "[".to_owned(),
            Self::Backslash => "\\".to_owned(),
            Self::RightBracket => "]".to_owned(),
            Self::Quote => "'".to_owned(),
            Self::Numpad(digit) => format!("Numpad{digit}"),
            Self::NumpadMultiply => "NumpadMultiply".to_owned(),
            Self::NumpadAdd => "NumpadAdd".to_owned(),
            Self::NumpadSubtract => "NumpadSubtract".to_owned(),
            Self::NumpadDivide => "NumpadDivide".to_owned(),
        }
    }
}

/// 解析热键字符串。空白在 `+` 两侧会被去掉。大小写不区分。
pub fn parse_hotkey(text: &str) -> Result<Hotkey, ConfigError> {
    let tokens: Vec<&str> = text
        .split('+')
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .collect();
    if tokens.is_empty() || tokens.len() != text.split('+').count() {
        return Err(ConfigError::InvalidHotkey);
    }
    let mut control = false;
    let mut alt = false;
    let mut shift = false;
    let mut win = false;
    let mut key = None;
    for token in tokens {
        match token.to_ascii_lowercase().as_str() {
            "ctrl" => control = true,
            "alt" => alt = true,
            "shift" => shift = true,
            "win" => win = true,
            other => {
                if key.is_some() {
                    return Err(ConfigError::InvalidHotkey);
                }
                key = Some(parse_key(other)?);
            }
        }
    }
    let Some(key) = key else {
        return Err(ConfigError::InvalidHotkey);
    };
    let hotkey = Hotkey {
        control,
        alt,
        shift,
        win,
        key,
    };
    if key.requires_modifier() && !hotkey.has_modifier() {
        return Err(ConfigError::InvalidHotkey);
    }
    Ok(hotkey)
}

fn parse_key(token: &str) -> Result<HotkeyKey, ConfigError> {
    let bytes = token.as_bytes();
    if bytes.len() == 1 {
        let byte = bytes[0].to_ascii_uppercase();
        if byte.is_ascii_uppercase() {
            return Ok(HotkeyKey::Letter(byte));
        }
        if byte.is_ascii_digit() {
            return Ok(HotkeyKey::Digit(byte));
        }
    }
    if let Some(key) = named_key(token) {
        return Ok(key);
    }
    let Some(rest) = token.strip_prefix('f') else {
        return Err(ConfigError::InvalidHotkey);
    };
    if rest.len() > 1 && rest.starts_with('0') {
        return Err(ConfigError::InvalidHotkey);
    }
    let index: u8 = rest.parse().map_err(|_| ConfigError::InvalidHotkey)?;
    if (1..=24).contains(&index) {
        Ok(HotkeyKey::Function(index))
    } else {
        Err(ConfigError::InvalidHotkey)
    }
}

fn named_key(token: &str) -> Option<HotkeyKey> {
    Some(match token {
        "space" => HotkeyKey::Space,
        "up" => HotkeyKey::Up,
        "down" => HotkeyKey::Down,
        "left" => HotkeyKey::Left,
        "right" => HotkeyKey::Right,
        "home" => HotkeyKey::Home,
        "end" => HotkeyKey::End,
        "pageup" => HotkeyKey::PageUp,
        "pagedown" => HotkeyKey::PageDown,
        "insert" => HotkeyKey::Insert,
        "delete" => HotkeyKey::Delete,
        ";" => HotkeyKey::Semicolon,
        "=" => HotkeyKey::Equals,
        "," => HotkeyKey::Comma,
        "-" => HotkeyKey::Minus,
        "." => HotkeyKey::Period,
        "/" => HotkeyKey::Slash,
        "`" => HotkeyKey::Backtick,
        "[" => HotkeyKey::LeftBracket,
        "\\" => HotkeyKey::Backslash,
        "]" => HotkeyKey::RightBracket,
        "'" => HotkeyKey::Quote,
        "numpad0" => HotkeyKey::Numpad(0),
        "numpad1" => HotkeyKey::Numpad(1),
        "numpad2" => HotkeyKey::Numpad(2),
        "numpad3" => HotkeyKey::Numpad(3),
        "numpad4" => HotkeyKey::Numpad(4),
        "numpad5" => HotkeyKey::Numpad(5),
        "numpad6" => HotkeyKey::Numpad(6),
        "numpad7" => HotkeyKey::Numpad(7),
        "numpad8" => HotkeyKey::Numpad(8),
        "numpad9" => HotkeyKey::Numpad(9),
        "numpadmultiply" => HotkeyKey::NumpadMultiply,
        "numpadadd" => HotkeyKey::NumpadAdd,
        "numpadsubtract" => HotkeyKey::NumpadSubtract,
        "numpaddivide" => HotkeyKey::NumpadDivide,
        _ => return None,
    })
}

/// 搜索条默认热键。产品规格写的是 `Ctrl+Alt+M`。
pub fn default_search_hotkey() -> String {
    Hotkey {
        control: true,
        alt: true,
        shift: false,
        win: false,
        key: HotkeyKey::Letter(b'M'),
    }
    .canonical()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_form_ignores_order_and_case() {
        let parsed = parse_hotkey(" alt + ctrl + m ").unwrap();
        assert_eq!(parsed.canonical(), "Ctrl+Alt+M");
        assert_eq!(parsed.virtual_key(), u32::from(b'M'));
        assert!(parsed.control && parsed.alt && !parsed.shift && !parsed.win);
    }

    #[test]
    fn function_keys_and_digits_parse() {
        assert_eq!(parse_hotkey("F1").unwrap().virtual_key(), 0x70);
        assert_eq!(parse_hotkey("f24").unwrap().canonical(), "F24");
        assert_eq!(
            parse_hotkey("Ctrl+0").unwrap().virtual_key(),
            u32::from(b'0')
        );
        assert!(parse_hotkey("F25").is_err());
        assert!(parse_hotkey("F01").is_err());
        assert!(parse_hotkey("Ctrl").is_err());
        assert!(parse_hotkey("Ctrl++M").is_err());
        assert!(parse_hotkey("Ctrl+Alt+M+K").is_err());
    }

    #[test]
    fn widened_keys_parse_with_a_modifier_and_space_alone_does_not() {
        assert_eq!(
            parse_hotkey("Ctrl+Space").unwrap().canonical(),
            "Ctrl+Space"
        );
        assert_eq!(parse_hotkey("ctrl+space").unwrap().virtual_key(), 0x20);
        assert_eq!(parse_hotkey("Alt+Left").unwrap().canonical(), "Alt+Left");
        assert_eq!(parse_hotkey("alt+LEFT").unwrap().virtual_key(), 0x25);
        assert_eq!(
            parse_hotkey("Ctrl+Shift+/").unwrap().canonical(),
            "Ctrl+Shift+/"
        );
        assert_eq!(parse_hotkey("Ctrl+Shift+/").unwrap().virtual_key(), 0xBF);
        assert_eq!(
            parse_hotkey("Ctrl+Numpad1").unwrap().canonical(),
            "Ctrl+Numpad1"
        );
        assert_eq!(parse_hotkey("ctrl+numpad1").unwrap().virtual_key(), 0x61);
        assert_eq!(
            parse_hotkey("space + shift + ctrl").unwrap().canonical(),
            "Ctrl+Shift+Space"
        );
        assert!(parse_hotkey("Space").is_err());
        assert!(parse_hotkey("space").is_err());
        assert!(parse_hotkey("Left").is_err());
        assert!(parse_hotkey("Numpad1").is_err());
        assert!(parse_hotkey("/").is_err());
    }

    #[test]
    fn previously_legal_hotkeys_still_parse() {
        assert_eq!(
            parse_hotkey("Ctrl+Alt+M").unwrap().canonical(),
            "Ctrl+Alt+M"
        );
        assert_eq!(
            parse_hotkey("ctrl+shift+f2").unwrap().canonical(),
            "Ctrl+Shift+F2"
        );
        assert_eq!(parse_hotkey("F12").unwrap().canonical(), "F12");
        assert_eq!(parse_hotkey("alt + 7").unwrap().canonical(), "Alt+7");
        assert_eq!(
            parse_hotkey("Win+Shift+A").unwrap().canonical(),
            "Shift+Win+A"
        );
        assert!(parse_hotkey("M").is_err());
        assert!(parse_hotkey("5").is_err());
    }

    #[test]
    fn every_virtual_key_round_trips_through_the_same_text() {
        let mut named = 0;
        for vk in 0..=0xFF {
            let Some(key) = HotkeyKey::from_virtual_key(vk) else {
                continue;
            };
            assert_eq!(key.virtual_key(), vk, "{vk:#x}");
            let hotkey = Hotkey {
                control: key.requires_modifier(),
                alt: false,
                shift: false,
                win: false,
                key,
            };
            let text = hotkey.canonical();
            let parsed = parse_hotkey(&text).unwrap();
            assert_eq!(parsed, hotkey, "{text}");
            let label = text.rsplit('+').next().unwrap();
            let folded = parse_hotkey(&format!("Ctrl+{}", label.to_ascii_lowercase())).unwrap();
            assert_eq!(folded.key, key, "{label}");
            if key.requires_modifier() {
                if !matches!(key, HotkeyKey::Letter(_) | HotkeyKey::Digit(_)) {
                    named += 1;
                }
                assert!(parse_hotkey(label).is_err(), "{label}");
            } else {
                assert_eq!(parse_hotkey(label).unwrap().key, key, "{label}");
            }
        }
        assert_eq!(named, 36, "空格、方向、导航、标点、小键盘数字和运算符");
    }
}
