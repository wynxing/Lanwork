//! 按 MS-SHLLINK 写 `.lnk`。
//!
//! 写本机路径的 IDList，再写 LinkInfo 里的本地路径，以及 Unicode 的工作目录和参数。
//! `IShellLinkW::GetPath` 读的是 IDList；只有 LinkInfo 时它返回空路径。
//! 文件末尾是 4 字节的 ExtraData 结束块。Windows 上由测试用 `IShellLinkW` 读回。

use crate::ToolError;

const HAS_LINK_TARGET_ID_LIST: u32 = 0x0000_0001;
const HAS_LINK_INFO: u32 = 0x0000_0002;
const HAS_WORKING_DIR: u32 = 0x0000_0010;
const HAS_ARGUMENTS: u32 = 0x0000_0020;
const IS_UNICODE: u32 = 0x0000_0080;
const HEADER_SIZE: usize = 76;
/// 规格示例里的 FILETIME。固定值，生成结果可重复。
const FIXED_FILETIME: u64 = 0x01C9_1515_F2EE_E9D0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShellLink {
    pub(crate) target: String,
    pub(crate) working_dir: String,
    pub(crate) arguments: String,
}

pub(crate) fn shell_path(path: &std::path::Path) -> String {
    let raw = path.to_string_lossy().replace('/', "\\");
    raw.strip_prefix(r"\\?\").unwrap_or(&raw).to_string()
}

pub(crate) fn write_shell_link(link: &ShellLink) -> Result<Vec<u8>, ToolError> {
    if link.target.contains('\0')
        || link.working_dir.contains('\0')
        || link.arguments.contains('\0')
    {
        return Err(ToolError::new("快捷方式路径或参数含有空字符"));
    }
    let mut buf = Vec::with_capacity(256);
    push_u32(&mut buf, HEADER_SIZE as u32);
    buf.extend_from_slice(&[
        0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x46,
    ]);
    let drive_target = is_drive_path(&link.target);
    let mut flags = HAS_LINK_INFO | HAS_WORKING_DIR | HAS_ARGUMENTS | IS_UNICODE;
    if drive_target {
        flags |= HAS_LINK_TARGET_ID_LIST;
    }
    push_u32(&mut buf, flags);
    push_u32(&mut buf, 0x20);
    push_u64(&mut buf, FIXED_FILETIME);
    push_u64(&mut buf, FIXED_FILETIME);
    push_u64(&mut buf, FIXED_FILETIME);
    push_u32(&mut buf, 0);
    push_u32(&mut buf, 0);
    push_u32(&mut buf, 1);
    push_u16(&mut buf, 0);
    push_u16(&mut buf, 0);
    push_u32(&mut buf, 0);
    push_u32(&mut buf, 0);
    debug_assert_eq!(buf.len(), HEADER_SIZE);

    if drive_target {
        buf.extend(id_list(&link.target)?);
    }
    buf.extend(link_info(&link.target)?);
    push_counted_utf16(&mut buf, &link.working_dir)?;
    push_counted_utf16(&mut buf, &link.arguments)?;
    push_u32(&mut buf, 0);
    Ok(buf)
}

#[cfg(test)]
pub(crate) fn parse_shell_link(bytes: &[u8]) -> Result<ShellLink, ToolError> {
    if bytes.len() < HEADER_SIZE {
        return Err(ToolError::new("快捷方式短于文件头"));
    }
    let header_size = read_u32(bytes, 0)?;
    if header_size as usize != HEADER_SIZE {
        return Err(ToolError::new("快捷方式文件头长度不是 76"));
    }
    let flags = read_u32(bytes, 0x14)?;
    if flags & HAS_LINK_INFO == 0 {
        return Err(ToolError::new("快捷方式没有 LinkInfo"));
    }
    let mut offset = HEADER_SIZE;
    if flags & 0x1 != 0 {
        let id_list_size = read_u16(bytes, offset)? as usize;
        offset += 2 + id_list_size;
    }
    let info_size = read_u32(bytes, offset)? as usize;
    if offset + info_size > bytes.len() {
        return Err(ToolError::new("LinkInfo 超出文件"));
    }
    let info = &bytes[offset..offset + info_size];
    let target = link_info_target(info)?;
    offset += info_size;
    let (working_dir, next) = read_counted_utf16(bytes, offset)?;
    let (arguments, _) = read_counted_utf16(bytes, next)?;
    Ok(ShellLink {
        target,
        working_dir,
        arguments,
    })
}

fn id_list(target: &str) -> Result<Vec<u8>, ToolError> {
    let (drive, components) = split_drive_path(target)?;
    let mut items = root_item();
    items.extend(drive_item(&drive));
    for (index, name) in components.iter().enumerate() {
        let directory = index + 1 != components.len();
        items.extend(fs_item(name, directory)?);
    }
    let mut out = Vec::with_capacity(items.len() + 4);
    let size =
        u16::try_from(items.len() + 2).map_err(|_| ToolError::new("快捷方式 IDList 过长"))?;
    push_u16(&mut out, size);
    out.extend(items);
    push_u16(&mut out, 0);
    Ok(out)
}

fn is_drive_path(target: &str) -> bool {
    let bytes = target.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

fn split_drive_path(target: &str) -> Result<(String, Vec<String>), ToolError> {
    if !is_drive_path(target) {
        return Err(ToolError::new(format!(
            "快捷方式目标不是本机盘符路径：{target}"
        )));
    }
    let drive = target[..3].to_string();
    let components = target[3..]
        .split('\\')
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if components.is_empty() {
        return Err(ToolError::new(format!("快捷方式目标没有文件名：{target}")));
    }
    Ok((drive, components))
}

fn root_item() -> Vec<u8> {
    // 本机「此电脑」。类型 0x1F，排序 0x50，CLSID 20D04FE0-3AEA-1069-A2D8-08002B30309D。
    let mut item = Vec::new();
    push_u16(&mut item, 0x14);
    item.extend_from_slice(&[
        0x1F, 0x50, 0xE0, 0x4F, 0xD0, 0x20, 0xEA, 0x3A, 0x69, 0x10, 0xA2, 0xD8, 0x08, 0x00, 0x2B,
        0x30, 0x30, 0x9D,
    ]);
    item
}

fn drive_item(drive: &str) -> Vec<u8> {
    let mut body = Vec::with_capacity(23);
    body.push(0x2F);
    body.extend_from_slice(drive.as_bytes());
    body.push(0);
    body.extend_from_slice(&[0u8; 18]);
    let mut item = Vec::with_capacity(body.len() + 2);
    push_u16(
        &mut item,
        u16::try_from(body.len() + 2).expect("drive item fits"),
    );
    item.extend(body);
    item
}

fn fs_item(name: &str, directory: bool) -> Result<Vec<u8>, ToolError> {
    if name.is_empty() || name.contains('\0') {
        return Err(ToolError::new("快捷方式路径组件为空"));
    }
    let long = is_long_filename(name);
    let mut type_flags = if directory { 0x31 } else { 0x32 };
    let short = if long {
        type_flags |= 0x04;
        short_filename(name)
    } else {
        name.to_string()
    };
    let mut body = vec![type_flags, 0];
    push_u32(&mut body, 0);
    push_u16(&mut body, 0);
    push_u16(&mut body, 0);
    push_u16(&mut body, if directory { 0x10 } else { 0x20 });
    if long {
        for unit in name.encode_utf16() {
            push_u16(&mut body, unit);
        }
        push_u16(&mut body, 0);
        body.extend(short.as_bytes());
        body.push(0);
    } else {
        body.extend(name.as_bytes());
        body.push(0);
        body.push(0);
    }
    let mut item = Vec::with_capacity(body.len() + 2);
    let size = u16::try_from(body.len() + 2).map_err(|_| ToolError::new("快捷方式路径组件过长"))?;
    push_u16(&mut item, size);
    item.extend(body);
    Ok(item)
}

fn is_long_filename(name: &str) -> bool {
    let Some(first) = name.chars().next() else {
        return true;
    };
    if first == '.' || name.ends_with('.') || !name.is_ascii() {
        return true;
    }
    let (base, ext) = split_filename(name);
    let awkward =
        |part: &str| part.contains(['.', '"', '/', '\\', '[', ']', ':', ';', '=', ',', ' ']);
    (name.contains('.') && (base.chars().count() > 8 || ext.chars().count() > 3))
        || (!name.contains('.') && name.chars().count() > 12)
        || awkward(&base)
        || awkward(&ext)
}

fn short_filename(name: &str) -> String {
    let trimmed = name.trim_matches('.');
    let (base, ext) = split_filename(trimmed);
    let fold = |part: &str, limit: usize| {
        let mut folded = String::new();
        for ch in part.chars() {
            if ch == ' ' {
                continue;
            }
            let mapped = if matches!(
                ch,
                '.' | '"' | '/' | '\\' | '[' | ']' | ':' | ';' | '=' | ',' | '+'
            ) {
                '_'
            } else {
                ch
            };
            folded.push(mapped);
        }
        folded.chars().take(limit).collect::<String>()
    };
    let mut short = fold(&base, 6);
    short.push_str("~1");
    let ext = fold(&ext, 3);
    if !ext.is_empty() {
        short.push('.');
        short.push_str(&ext);
    }
    short
        .chars()
        .map(|ch| {
            if ch.is_ascii() {
                ch.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

fn split_filename(name: &str) -> (String, String) {
    match name.rfind('.') {
        Some(index) if index > 0 => (name[..index].to_string(), name[index + 1..].to_string()),
        _ => (name.to_string(), String::new()),
    }
}

fn link_info(target: &str) -> Result<Vec<u8>, ToolError> {
    let unicode = !target.is_ascii();
    let header_size: u32 = if unicode { 0x24 } else { 0x1C };
    let mut body = vec![0u8; header_size as usize];

    let volume_off = body.len() as u32;
    let volume_start = body.len();
    push_u32(&mut body, 0);
    push_u32(&mut body, 3);
    push_u32(&mut body, 0);
    push_u32(&mut body, 0x10);
    body.push(0);
    let volume_size =
        u32::try_from(body.len() - volume_start).map_err(|_| ToolError::new("VolumeID 过长"))?;
    body[volume_start..volume_start + 4].copy_from_slice(&volume_size.to_le_bytes());

    let local_off = u32::try_from(body.len()).map_err(|_| ToolError::new("LinkInfo 过长"))?;
    body.extend(ansi_lossy(target));
    body.push(0);

    let suffix_off = u32::try_from(body.len()).map_err(|_| ToolError::new("LinkInfo 过长"))?;
    body.push(0);

    let (local_unicode_off, suffix_unicode_off) = if unicode {
        let local_unicode_off =
            u32::try_from(body.len()).map_err(|_| ToolError::new("LinkInfo 过长"))?;
        for unit in target.encode_utf16() {
            push_u16(&mut body, unit);
        }
        push_u16(&mut body, 0);
        let suffix_unicode_off =
            u32::try_from(body.len()).map_err(|_| ToolError::new("LinkInfo 过长"))?;
        push_u16(&mut body, 0);
        (local_unicode_off, suffix_unicode_off)
    } else {
        (0, 0)
    };

    let total = u32::try_from(body.len()).map_err(|_| ToolError::new("LinkInfo 过长"))?;
    body[0..4].copy_from_slice(&total.to_le_bytes());
    body[4..8].copy_from_slice(&header_size.to_le_bytes());
    body[8..12].copy_from_slice(&1u32.to_le_bytes());
    body[12..16].copy_from_slice(&volume_off.to_le_bytes());
    body[16..20].copy_from_slice(&local_off.to_le_bytes());
    body[20..24].copy_from_slice(&0u32.to_le_bytes());
    body[24..28].copy_from_slice(&suffix_off.to_le_bytes());
    if unicode {
        body[28..32].copy_from_slice(&local_unicode_off.to_le_bytes());
        body[32..36].copy_from_slice(&suffix_unicode_off.to_le_bytes());
    }
    Ok(body)
}

#[cfg(test)]
fn link_info_target(info: &[u8]) -> Result<String, ToolError> {
    if info.len() < 0x1C {
        return Err(ToolError::new("LinkInfo 过短"));
    }
    let header_size = read_u32(info, 4)?;
    let local_off = read_u32(info, 16)? as usize;
    let ansi = read_cstr(info, local_off)?;
    if header_size >= 0x24 {
        let unicode_off = read_u32(info, 28)? as usize;
        let unicode = read_utf16_cstr(info, unicode_off)?;
        if !unicode.is_empty() {
            return Ok(unicode);
        }
    }
    Ok(ansi)
}

fn ansi_lossy(text: &str) -> Vec<u8> {
    text.chars()
        .map(|ch| if ch.is_ascii() { ch as u8 } else { b'?' })
        .collect()
}

fn push_counted_utf16(buf: &mut Vec<u8>, text: &str) -> Result<(), ToolError> {
    let units: Vec<u16> = text.encode_utf16().collect();
    let count = u16::try_from(units.len())
        .map_err(|_| ToolError::new("快捷方式字符串超过 65535 个 UTF-16 单元"))?;
    push_u16(buf, count);
    for unit in units {
        push_u16(buf, unit);
    }
    Ok(())
}

#[cfg(test)]
fn read_counted_utf16(bytes: &[u8], offset: usize) -> Result<(String, usize), ToolError> {
    let count = read_u16(bytes, offset)? as usize;
    let start = offset + 2;
    let end = start + count * 2;
    if end > bytes.len() {
        return Err(ToolError::new("快捷方式字符串超出文件"));
    }
    let mut units = Vec::with_capacity(count);
    for index in 0..count {
        units.push(read_u16(bytes, start + index * 2)?);
    }
    let text =
        String::from_utf16(&units).map_err(|_| ToolError::new("快捷方式字符串不是合法 UTF-16"))?;
    Ok((text, end))
}

#[cfg(test)]
fn read_cstr(bytes: &[u8], offset: usize) -> Result<String, ToolError> {
    let rest = bytes
        .get(offset..)
        .ok_or_else(|| ToolError::new("字符串偏移超出 LinkInfo"))?;
    let end = rest
        .iter()
        .position(|byte| *byte == 0)
        .ok_or_else(|| ToolError::new("ANSI 路径没有结束符"))?;
    let text = rest[..end].iter().map(|byte| char::from(*byte)).collect();
    Ok(text)
}

#[cfg(test)]
fn read_utf16_cstr(bytes: &[u8], offset: usize) -> Result<String, ToolError> {
    if offset > bytes.len() {
        return Err(ToolError::new("Unicode 路径偏移超出 LinkInfo"));
    }
    let mut units = Vec::new();
    let mut cursor = offset;
    while cursor + 1 < bytes.len() {
        let unit = read_u16(bytes, cursor)?;
        cursor += 2;
        if unit == 0 {
            return String::from_utf16(&units)
                .map_err(|_| ToolError::new("Unicode 路径不是合法 UTF-16"));
        }
        units.push(unit);
    }
    Err(ToolError::new("Unicode 路径没有结束符"))
}

fn push_u16(buf: &mut Vec<u8>, value: u16) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(buf: &mut Vec<u8>, value: u32) {
    buf.extend_from_slice(&value.to_le_bytes());
}

fn push_u64(buf: &mut Vec<u8>, value: u64) {
    buf.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ToolError> {
    let chunk = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| ToolError::new("读取超出快捷方式文件"))?;
    Ok(u16::from_le_bytes(chunk.try_into().expect("2 bytes")))
}

#[cfg(test)]
fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ToolError> {
    let chunk = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| ToolError::new("读取超出快捷方式文件"))?;
    Ok(u32::from_le_bytes(chunk.try_into().expect("4 bytes")))
}

#[cfg(all(test, windows))]
fn load_with_ishelllink(path: &std::path::Path) -> Result<ShellLink, ToolError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        IPersistFile, STGM_READ,
    };
    use windows::Win32::UI::Shell::{
        IShellLinkW, SLGP_RAWPATH, SLR_NO_UI, SLR_NOUPDATE, ShellLink as ShellLinkClsid,
    };
    use windows::core::{Interface, PCWSTR};

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|err| ToolError::new(format!("CoInitializeEx：{err}")))?;
        let link: IShellLinkW = CoCreateInstance(
            &ShellLinkClsid,
            None::<&windows::core::IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
        .map_err(|err| ToolError::new(format!("创建 IShellLinkW：{err}")))?;
        let persist: IPersistFile = link
            .cast()
            .map_err(|err| ToolError::new(format!("IPersistFile：{err}")))?;
        persist
            .Load(PCWSTR(wide.as_ptr()), STGM_READ)
            .map_err(|err| ToolError::new(format!("读取快捷方式：{err}")))?;
        let _ = link.Resolve(
            HWND(std::ptr::null_mut()),
            (SLR_NO_UI.0 | SLR_NOUPDATE.0) as u32,
        );
        let mut target = vec![0u16; 32_768];
        let mut working_dir = vec![0u16; 32_768];
        let mut arguments = vec![0u16; 32_768];
        link.GetPath(&mut target, std::ptr::null_mut(), SLGP_RAWPATH.0 as u32)
            .map_err(|err| ToolError::new(format!("GetPath：{err}")))?;
        link.GetWorkingDirectory(&mut working_dir)
            .map_err(|err| ToolError::new(format!("GetWorkingDirectory：{err}")))?;
        link.GetArguments(&mut arguments)
            .map_err(|err| ToolError::new(format!("GetArguments：{err}")))?;
        Ok(ShellLink {
            target: utf16_buf(&target),
            working_dir: utf16_buf(&working_dir),
            arguments: utf16_buf(&arguments),
        })
    }
}

#[cfg(all(test, windows))]
fn utf16_buf(buf: &[u16]) -> String {
    let end = buf.iter().position(|unit| *unit == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

#[cfg(test)]
fn link_info_offset(raw: &[u8]) -> usize {
    let flags = u32::from_le_bytes(raw[0x14..0x18].try_into().expect("flags"));
    assert_ne!(flags & HAS_LINK_TARGET_ID_LIST, 0, "快捷方式没有 IDList");
    let list_size = u16::from_le_bytes(
        raw[HEADER_SIZE..HEADER_SIZE + 2]
            .try_into()
            .expect("idlist"),
    ) as usize;
    HEADER_SIZE + 2 + list_size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_link_roundtrip_matches_spec_layout() {
        let raw = write_shell_link(&ShellLink {
            target: r"C:\test\a.txt".to_string(),
            working_dir: r"C:\test".to_string(),
            arguments: "fixture".to_string(),
        })
        .unwrap();
        assert_eq!(&raw[..4], &0x4Cu32.to_le_bytes());
        assert_eq!(
            &raw[4..20],
            &[
                0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x46
            ]
        );
        let info_at = link_info_offset(&raw);
        let info_size = u32::from_le_bytes(raw[info_at..info_at + 4].try_into().unwrap());
        assert_eq!(info_size, 0x3C);
        let local_off = u32::from_le_bytes(raw[info_at + 16..info_at + 20].try_into().unwrap());
        assert_eq!(local_off, 0x2D);
        let parsed = parse_shell_link(&raw).unwrap();
        assert_eq!(parsed.target, r"C:\test\a.txt");
        assert_eq!(parsed.working_dir, r"C:\test");
        assert_eq!(parsed.arguments, "fixture");
    }

    #[test]
    fn unicode_target_roundtrip() {
        let raw = write_shell_link(&ShellLink {
            target: r"C:\测量\应用.exe".to_string(),
            working_dir: r"C:\测量".to_string(),
            arguments: "参数".to_string(),
        })
        .unwrap();
        let info_at = link_info_offset(&raw);
        let header_size = u32::from_le_bytes(raw[info_at + 4..info_at + 8].try_into().unwrap());
        assert_eq!(header_size, 0x24);
        let parsed = parse_shell_link(&raw).unwrap();
        assert_eq!(parsed.target, r"C:\测量\应用.exe");
        assert_eq!(parsed.working_dir, r"C:\测量");
        assert_eq!(parsed.arguments, "参数");
    }

    #[test]
    fn strips_verbatim_prefix() {
        let path = std::path::Path::new(r"\\?\C:\work\app.exe");
        assert_eq!(shell_path(path), r"C:\work\app.exe");
    }

    #[cfg(windows)]
    fn assert_same_path(actual: &str, expected: &std::path::Path) {
        assert!(!actual.is_empty(), "IShellLink 没有返回路径：{expected:?}");
        let expected_text = shell_path(expected);
        if actual.eq_ignore_ascii_case(&expected_text) {
            return;
        }
        let left = std::fs::canonicalize(actual)
            .unwrap_or_else(|err| panic!("无法解析 IShellLink 路径 {actual}：{err}"));
        let right = std::fs::canonicalize(expected)
            .unwrap_or_else(|err| panic!("无法解析期望路径 {expected:?}：{err}"));
        assert_eq!(
            left, right,
            "IShellLink 返回 {actual}，写入的是 {expected_text}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn ishelllink_reads_fixture_and_unicode_shortcuts() {
        let root = std::env::temp_dir().join(format!(
            "lanwork-lnk-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let report = crate::generate(&crate::GenerateRequest {
            out: root.join("fixture"),
            scale: crate::Scale {
                todos: 0,
                lists: 0,
                notes: 0,
                note_bytes: 0,
                shelves: 0,
                refs: 0,
                shortcuts: 1,
            },
            user_profile: None,
        })
        .unwrap();
        let loaded = load_with_ishelllink(&report.shortcuts_dir.join("App 0001.lnk")).unwrap();
        assert_same_path(
            &loaded.target,
            &report.shortcuts_dir.join("targets").join("App 0001.exe"),
        );
        assert_eq!(loaded.arguments, "fixture");
        assert_same_path(&loaded.working_dir, &report.shortcuts_dir.join("targets"));

        let folder = root.join("测量");
        std::fs::create_dir_all(&folder).unwrap();
        let target = folder.join("应用.exe");
        std::fs::write(&target, b"").unwrap();
        let link_path = root.join("unicode.lnk");
        let bytes = write_shell_link(&ShellLink {
            target: shell_path(&target),
            working_dir: shell_path(&folder),
            arguments: "参数".to_string(),
        })
        .unwrap();
        std::fs::write(&link_path, bytes).unwrap();
        let loaded = load_with_ishelllink(&link_path).unwrap();
        assert_same_path(&loaded.target, &target);
        assert_same_path(&loaded.working_dir, &folder);
        assert_eq!(loaded.arguments, "参数");
        let _ = std::fs::remove_dir_all(root);
    }
}
