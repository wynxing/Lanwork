//! `%LOCALAPPDATA%\Lanwork\cache\icons` 里的解码图标。
//!
//! 文件名是路径和图标序号的 SHA-256，加 `.bin`。源文件的修改时间和长度写在文件头里，
//! 对不上就当作没有这份副本。取图失败不写文件。读到坏文件就删掉。

use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

use sha2::{Digest, Sha256};

use super::icon::RgbaImage;
use super::model::IconKey;
use crate::storage::{self, atomic_write};

/// 缓存目录下的子目录，与 `apps.json` 同级。
pub const ICON_DISK_DIR: &str = "icons";

const MAGIC: &[u8] = b"LWIC";
const CACHE_VERSION: u32 = 1;
const HEADER_LEN: usize = 40;
/// 与壳层取图的边长上限一致。
const MAX_EDGE: u32 = 256;

/// 源文件当时的修改时间和长度。路径不是文件时 `present` 为假，仍可以缓存。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourceStamp {
    present: bool,
    secs: i64,
    nanos: u32,
    len: u64,
}

impl SourceStamp {
    fn absent() -> Self {
        Self {
            present: false,
            secs: 0,
            nanos: 0,
            len: 0,
        }
    }
}

/// 按路径读修改时间和长度。读不到时返回「没有文件」的标记，调用方仍可缓存这次取到的图。
pub(crate) fn source_stamp(path: &str) -> SourceStamp {
    if path.is_empty() {
        return SourceStamp::absent();
    }
    let Ok(meta) = std::fs::metadata(path) else {
        return SourceStamp::absent();
    };
    let Ok(modified) = meta.modified() else {
        return SourceStamp::absent();
    };
    let Ok(duration) = modified.duration_since(UNIX_EPOCH) else {
        return SourceStamp::absent();
    };
    SourceStamp {
        present: true,
        secs: i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        nanos: duration.subsec_nanos(),
        len: meta.len(),
    }
}

/// 把一个时间写成文件的修改时间。测试用来制造和读取时不同的标记。
#[cfg(test)]
pub(crate) fn set_modified(path: &Path, secs: u64) {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .expect("open stamp file");
    let when = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
    file.set_modified(when).expect("set modified");
}

#[derive(Debug, Clone)]
pub(crate) struct IconDisk {
    dir: PathBuf,
}

impl IconDisk {
    pub(crate) fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// 源文件标记和文件头一致时返回图。坏文件删掉。标记对不上时留下文件，下次成功取图再覆盖。
    pub(crate) fn load(&self, key: &IconKey) -> Option<(RgbaImage, SourceStamp)> {
        let path = self.path_for(key)?;
        let bytes = storage::fs_read(&path).ok()?;
        let current = source_stamp(&key.path);
        match decode(&bytes) {
            Some((image, stamp)) if stamp == current => Some((image, stamp)),
            Some(_) => None,
            None => {
                self.discard(&path);
                None
            }
        }
    }

    /// 写盘失败时返回 false。调用方仍可以把图留在内存里。
    pub(crate) fn store(&self, key: &IconKey, image: &RgbaImage, stamp: SourceStamp) -> bool {
        let Some(path) = self.path_for(key) else {
            return false;
        };
        let Some(bytes) = encode(image, stamp) else {
            return false;
        };
        if storage::fs_create_dir_all(&self.dir).is_err() {
            return false;
        }
        atomic_write(&path, &bytes).is_ok()
    }

    pub(crate) fn path_for(&self, key: &IconKey) -> Option<PathBuf> {
        let name = cache_file_name(key)?;
        let path = self.dir.join(name);
        if !self.contains_cache_file(&path) {
            return None;
        }
        Some(path)
    }

    fn discard(&self, path: &Path) {
        if !self.contains_cache_file(path) {
            return;
        }
        let _ = storage::fs_remove_file(path);
    }

    fn contains_cache_file(&self, path: &Path) -> bool {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        if !is_cache_file_name(name) {
            return false;
        }
        if path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return false;
        }
        path.parent() == Some(self.dir.as_path())
    }
}

fn cache_file_name(key: &IconKey) -> Option<String> {
    if key.path.len() > 32_768 {
        return None;
    }
    let mut material = Vec::with_capacity(key.path.len() + 5);
    material.extend_from_slice(key.path.as_bytes());
    material.push(0);
    material.extend_from_slice(&key.index.to_le_bytes());
    let digest = Sha256::digest(&material);
    let mut name = String::with_capacity(68);
    for byte in digest {
        name.push(char::from(b"0123456789abcdef"[(byte >> 4) as usize]));
        name.push(char::from(b"0123456789abcdef"[(byte & 0x0f) as usize]));
    }
    name.push_str(".bin");
    if !is_cache_file_name(&name) {
        return None;
    }
    Some(name)
}

fn is_cache_file_name(name: &str) -> bool {
    let Some(stem) = name.strip_suffix(".bin") else {
        return false;
    };
    stem.len() == 64
        && stem
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn encode(image: &RgbaImage, stamp: SourceStamp) -> Option<Vec<u8>> {
    if image.width > MAX_EDGE || image.height > MAX_EDGE {
        return None;
    }
    let mut out = Vec::with_capacity(HEADER_LEN + image.pixels.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    out.extend_from_slice(&image.width.to_le_bytes());
    out.extend_from_slice(&image.height.to_le_bytes());
    out.extend_from_slice(&stamp.len.to_le_bytes());
    out.extend_from_slice(&stamp.secs.to_le_bytes());
    out.extend_from_slice(&stamp.nanos.to_le_bytes());
    out.push(u8::from(stamp.present));
    out.extend_from_slice(&[0, 0, 0]);
    debug_assert_eq!(out.len(), HEADER_LEN);
    out.extend_from_slice(&image.pixels);
    Some(out)
}

fn decode(bytes: &[u8]) -> Option<(RgbaImage, SourceStamp)> {
    if bytes.len() < HEADER_LEN || !bytes.starts_with(MAGIC) {
        return None;
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    if version != CACHE_VERSION {
        return None;
    }
    let width = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let height = u32::from_le_bytes(bytes[12..16].try_into().ok()?);
    if width == 0 || height == 0 || width > MAX_EDGE || height > MAX_EDGE {
        return None;
    }
    let len = u64::from_le_bytes(bytes[16..24].try_into().ok()?);
    let secs = i64::from_le_bytes(bytes[24..32].try_into().ok()?);
    let nanos = u32::from_le_bytes(bytes[32..36].try_into().ok()?);
    let present = bytes[36] == 1;
    if bytes[37] != 0 || bytes[38] != 0 || bytes[39] != 0 {
        return None;
    }
    let pixels = bytes[HEADER_LEN..].to_vec();
    let image = RgbaImage::new(width, height, pixels)?;
    let stamp = if present {
        SourceStamp {
            present: true,
            secs,
            nanos,
            len,
        }
    } else {
        SourceStamp::absent()
    };
    Some((image, stamp))
}

#[cfg(test)]
mod tests {
    use super::{IconDisk, source_stamp};
    use crate::dispatch::RgbaImage;
    use crate::dispatch::model::IconKey;
    use crate::storage::test_temp::TempDir;

    fn key(path: &str, index: i32) -> IconKey {
        IconKey {
            path: path.to_owned(),
            index,
        }
    }

    fn pixel(mark: u8) -> RgbaImage {
        RgbaImage::new(1, 1, vec![mark, 2, 3, 255]).unwrap()
    }

    #[test]
    fn roundtrip_keeps_path_and_index_apart() {
        let temp = TempDir::new();
        let file = temp.path().join("app.exe");
        std::fs::write(&file, b"v1").unwrap();
        super::set_modified(&file, 1_700_000_000);
        let disk = IconDisk::new(temp.path().join("icons"));
        let path = file.display().to_string();
        let first = key(&path, 0);
        let second = key(&path, 2);
        let stamp = source_stamp(&path);
        assert!(stamp.present);
        assert!(disk.store(&first, &pixel(1), stamp));
        assert!(disk.store(&second, &pixel(9), stamp));
        let (image, loaded) = disk.load(&first).expect("first index");
        assert_eq!(image, pixel(1));
        assert_eq!(loaded, stamp);
        assert_eq!(disk.load(&second).unwrap().0, pixel(9));
        assert_ne!(disk.path_for(&first), disk.path_for(&second));
    }

    #[test]
    fn a_changed_source_file_does_not_load() {
        let temp = TempDir::new();
        let file = temp.path().join("app.exe");
        std::fs::write(&file, b"v1").unwrap();
        super::set_modified(&file, 1_700_000_000);
        let disk = IconDisk::new(temp.path().join("icons"));
        let path = file.display().to_string();
        let icon = key(&path, 0);
        let stamp = source_stamp(&path);
        assert!(disk.store(&icon, &pixel(1), stamp));
        std::fs::write(&file, b"v2-longer").unwrap();
        super::set_modified(&file, 1_700_000_100);
        assert!(disk.load(&icon).is_none());
        assert!(disk.path_for(&icon).unwrap().is_file());
    }

    #[test]
    fn a_corrupt_file_is_removed() {
        let temp = TempDir::new();
        let disk = IconDisk::new(temp.path().join("icons"));
        let icon = key("shell:AppsFolder\\Example", 0);
        let path = disk.path_for(&icon).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"not-an-icon").unwrap();
        assert!(disk.load(&icon).is_none());
        assert!(!path.exists());
    }

    #[test]
    fn a_missing_path_still_roundtrips() {
        let temp = TempDir::new();
        let disk = IconDisk::new(temp.path().join("icons"));
        let icon = key(r"shell:AppsFolder\Contoso", 0);
        let stamp = source_stamp(&icon.path);
        assert!(!stamp.present);
        assert!(disk.store(&icon, &pixel(4), stamp));
        assert_eq!(disk.load(&icon).unwrap().0, pixel(4));
    }
}
