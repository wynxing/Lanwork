//! 热键字符串。
//!
//! 这是写入 `config.json` 的格式，不是设置页的交互。产品规格没有写键名表。
//! 当前接受 `Ctrl`、`Alt`、`Shift`、`Win` 和一个键。键是 `A`–`Z`、`0`–`9` 或 `F1`–`F24`。
//! 其他键名不接受。比较之前收成 `Ctrl+Alt+Shift+Win+键`。

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HotkeyKey {
    Letter(u8),
    Digit(u8),
    Function(u8),
}

impl Hotkey {
    /// Win32 虚拟键码。字母和数字用 ASCII 大写，功能键从 `VK_F1`（0x70）起。
    #[must_use]
    pub fn virtual_key(self) -> u32 {
        match self.key {
            HotkeyKey::Letter(letter) | HotkeyKey::Digit(letter) => u32::from(letter),
            HotkeyKey::Function(index) => 0x70 + u32::from(index - 1),
        }
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
}

impl HotkeyKey {
    fn label(self) -> String {
        match self {
            Self::Letter(letter) | Self::Digit(letter) => char::from(letter).to_string(),
            Self::Function(index) => format!("F{index}"),
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
    Ok(Hotkey {
        control,
        alt,
        shift,
        win,
        key,
    })
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
        return Err(ConfigError::InvalidHotkey);
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
        assert!(parse_hotkey("Ctrl+Space").is_err());
    }
}
