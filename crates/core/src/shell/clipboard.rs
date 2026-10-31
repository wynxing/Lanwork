//! 资源管理器「复制」放进剪贴板的字节。
//!
//! 真正的 `OpenClipboard` 在 `crates/app`。这里只排 `CF_HDROP`、`Preferred DropEffect`
//! 和 `CF_UNICODETEXT`，好让粘贴进资源管理器复制这一条，粘贴进文本框得到路径。

/// `CFSTR_PREFERREDDROPEFFECT`。值是小端 `DWORD`。
pub const PREFERRED_DROP_EFFECT_FORMAT: &str = "Preferred DropEffect";

/// `DROPEFFECT_COPY`。资源管理器据此把粘贴当成复制。
pub const DROPEFFECT_COPY: u32 = 1;

/// `DROPFILES` 头的字节数。`pFiles` 指向紧接在头后面的宽字符路径。
const DROPFILES_SIZE: usize = 20;

/// 一次「复制文件」要放上剪贴板的三份数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplorerFileCopy {
    /// `CF_HDROP`。`fWide` 为真，路径是 UTF-16，列表以两个 NUL 结束。
    pub hdrop: Vec<u8>,
    /// `Preferred DropEffect`，四个字节，小端 [`DROPEFFECT_COPY`]。
    pub preferred_drop_effect: [u8; 4],
    /// `CF_UNICODETEXT`。路径的 UTF-16，带一个结尾 NUL。
    pub unicode_text: Vec<u8>,
}

/// 路径可以放进剪贴板时返回这三份数据。空路径或含 NUL 的路径放不进去。
#[must_use]
pub fn explorer_file_copy(path: &str) -> Option<ExplorerFileCopy> {
    let unicode_text = unicode_clipboard_text(path)?;
    let mut hdrop = Vec::with_capacity(DROPFILES_SIZE + unicode_text.len() + 2);
    hdrop.extend((DROPFILES_SIZE as u32).to_le_bytes());
    hdrop.extend(0i32.to_le_bytes());
    hdrop.extend(0i32.to_le_bytes());
    hdrop.extend(0u32.to_le_bytes());
    hdrop.extend(1u32.to_le_bytes());
    hdrop.extend(&unicode_text);
    hdrop.extend(0u16.to_le_bytes());
    Some(ExplorerFileCopy {
        hdrop,
        preferred_drop_effect: DROPEFFECT_COPY.to_le_bytes(),
        unicode_text,
    })
}

/// `CF_UNICODETEXT` 的字节。空文本或含 NUL 时为 `None`。
#[must_use]
pub fn unicode_clipboard_text(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || text.contains('\0') {
        return None;
    }
    let mut bytes = Vec::with_capacity(text.len() * 2 + 2);
    for unit in text.encode_utf16() {
        bytes.extend(unit.to_le_bytes());
    }
    bytes.extend(0u16.to_le_bytes());
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::{DROPEFFECT_COPY, DROPFILES_SIZE, explorer_file_copy, unicode_clipboard_text};

    #[test]
    fn explorer_copy_is_a_wide_hdrop_with_copy_effect() {
        let path = "C:/Docs/a.txt";
        let copy = explorer_file_copy(path).expect("path");
        assert_eq!(
            u32::from_le_bytes(copy.hdrop[0..4].try_into().unwrap()),
            DROPFILES_SIZE as u32
        );
        assert_eq!(&copy.hdrop[4..16], &[0; 12]);
        assert_eq!(
            u32::from_le_bytes(copy.hdrop[16..20].try_into().unwrap()),
            1,
            "fWide"
        );
        let text = unicode_clipboard_text(path).unwrap();
        assert_eq!(
            &copy.hdrop[DROPFILES_SIZE..DROPFILES_SIZE + text.len()],
            &text
        );
        assert_eq!(
            &copy.hdrop[DROPFILES_SIZE + text.len()..],
            &0u16.to_le_bytes()
        );
        assert_eq!(copy.preferred_drop_effect, DROPEFFECT_COPY.to_le_bytes());
        assert_eq!(copy.unicode_text, text);
    }

    #[test]
    fn unicode_path_is_utf16() {
        let path = "C:/文档/a.txt";
        let copy = explorer_file_copy(path).expect("path");
        let units: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let mut bytes = Vec::new();
        for unit in units {
            bytes.extend(unit.to_le_bytes());
        }
        assert_eq!(copy.unicode_text, bytes);
    }

    #[test]
    fn empty_or_nul_paths_are_not_copied() {
        assert!(explorer_file_copy("").is_none());
        assert!(explorer_file_copy("C:/a\0b").is_none());
        assert!(unicode_clipboard_text("").is_none());
        assert!(unicode_clipboard_text("http://a\0b").is_none());
    }
}
