//! 数据目录 `cache/web-icons/` 里的 RGBA 缓存。
//!
//! 文件名只能是 64 位小写十六进制加 `.bin`。路径一旦离开这个目录，不读、不写、不删。

use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::dispatch::RgbaImage;
use crate::storage::{self, atomic_write};

/// 数据目录下的相对路径。可整份删除后重建，不进入导出和备份。
pub const WEB_ICON_CACHE_DIR: &str = "cache/web-icons";

/// 放进缓存和界面的边长上限。
pub const WEB_ICON_MAX_EDGE: u32 = 256;

const MAGIC: &[u8] = b"LWFI";
const CACHE_VERSION: u32 = 1;

/// 数据目录里的网站图标缓存。文件可以删，删了会再取。
#[derive(Debug, Clone)]
pub struct WebIconCache {
    dir: PathBuf,
}

impl WebIconCache {
    #[must_use]
    pub fn new(data_dir: &Path) -> Self {
        Self {
            dir: data_dir.join(WEB_ICON_CACHE_DIR),
        }
    }

    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    #[must_use]
    pub fn load(&self, normalized_key: &str) -> Option<RgbaImage> {
        let path = self.path_for(normalized_key)?;
        let bytes = storage::fs_read(&path).ok()?;
        match decode_cache(&bytes) {
            Some(image) => Some(image),
            None => {
                self.discard(&path);
                None
            }
        }
    }

    /// 写盘失败时返回 false。调用方仍可以先把图显示出来。
    pub fn store(&self, normalized_key: &str, image: &RgbaImage) -> bool {
        let Some(path) = self.path_for(normalized_key) else {
            return false;
        };
        let Some(bytes) = encode_cache(image) else {
            return false;
        };
        if storage::fs_create_dir_all(&self.dir).is_err() {
            return false;
        }
        atomic_write(&path, &bytes).is_ok()
    }

    /// 这个键对应的缓存文件。不在 `cache/web-icons` 里时返回 `None`。
    #[must_use]
    pub(crate) fn path_for(&self, normalized_key: &str) -> Option<PathBuf> {
        let name = cache_file_name(normalized_key)?;
        let path = self.dir.join(name);
        if !self.contains_cache_file(&path) {
            return None;
        }
        Some(path)
    }

    /// 只删除本目录下的缓存文件。其它路径原样留下。
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

fn cache_file_name(normalized_key: &str) -> Option<String> {
    if normalized_key.is_empty() || normalized_key.len() > 16_384 {
        return None;
    }
    let digest = Sha256::digest(normalized_key.as_bytes());
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

fn encode_cache(image: &RgbaImage) -> Option<Vec<u8>> {
    if image.width > WEB_ICON_MAX_EDGE || image.height > WEB_ICON_MAX_EDGE {
        return None;
    }
    let mut out = Vec::with_capacity(16 + image.pixels.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&CACHE_VERSION.to_le_bytes());
    out.extend_from_slice(&image.width.to_le_bytes());
    out.extend_from_slice(&image.height.to_le_bytes());
    out.extend_from_slice(&image.pixels);
    Some(out)
}

fn decode_cache(bytes: &[u8]) -> Option<RgbaImage> {
    if bytes.len() < 16 || !bytes.starts_with(MAGIC) {
        return None;
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().ok()?);
    if version != CACHE_VERSION {
        return None;
    }
    let width = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let height = u32::from_le_bytes(bytes[12..16].try_into().ok()?);
    if width == 0 || height == 0 || width > WEB_ICON_MAX_EDGE || height > WEB_ICON_MAX_EDGE {
        return None;
    }
    let pixels = bytes[16..].to_vec();
    RgbaImage::new(width, height, pixels)
}

#[cfg(test)]
mod tests {
    use super::{WEB_ICON_CACHE_DIR, WebIconCache};
    use crate::dispatch::RgbaImage;
    use crate::storage::test_temp::TempDir;
    use std::path::{Component, Path};

    fn temp_data() -> (TempDir, std::path::PathBuf) {
        let temp = TempDir::new();
        let data = temp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        (temp, data)
    }

    fn stays_inside(data_dir: &Path, path: &Path, cache_dir: &Path) {
        assert_eq!(path.parent(), Some(cache_dir));
        assert!(path.starts_with(data_dir.join(WEB_ICON_CACHE_DIR)));
        assert!(
            !path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        );
        let name = path.file_name().and_then(|name| name.to_str()).unwrap();
        assert!(name.ends_with(".bin"));
        assert!(!name.contains(".."));
        assert!(!name.contains('/') && !name.contains('\\'));
    }

    #[test]
    fn paths_stay_in_web_icons_and_outside_files_are_not_removed() {
        let (_temp, data) = temp_data();
        let cache = WebIconCache::new(&data);
        let key = "https://example.com/a";
        let path = cache.path_for(key).unwrap();
        stays_inside(&data, &path, cache.dir());

        let sneaky = cache.path_for("../../shelves/group.json").unwrap();
        stays_inside(&data, &sneaky, cache.dir());
        assert_ne!(sneaky.file_name(), Some(std::ffi::OsStr::new("group.json")));

        let shelf = data.join("shelves").join("group.json");
        std::fs::create_dir_all(shelf.parent().unwrap()).unwrap();
        std::fs::write(&shelf, b"shelf").unwrap();
        let beside = data.join("keep.bin");
        std::fs::write(&beside, b"keep").unwrap();
        std::fs::create_dir_all(cache.dir()).unwrap();
        let stray = cache.dir().join("notes.txt");
        std::fs::write(&stray, b"stay").unwrap();
        std::fs::write(&path, b"not-a-cache").unwrap();

        assert!(cache.load(key).is_none());
        assert!(!path.exists());
        assert_eq!(std::fs::read(&shelf).unwrap(), b"shelf");
        assert_eq!(std::fs::read(&beside).unwrap(), b"keep");
        assert_eq!(std::fs::read(&stray).unwrap(), b"stay");

        let escape = data.join("escape.bin");
        std::fs::write(&escape, b"escape").unwrap();
        cache.discard(&shelf);
        cache.discard(&beside);
        cache.discard(&stray);
        cache.discard(&cache.dir().join("..").join("escape.bin"));
        assert_eq!(std::fs::read(&shelf).unwrap(), b"shelf");
        assert_eq!(std::fs::read(&beside).unwrap(), b"keep");
        assert_eq!(std::fs::read(&stray).unwrap(), b"stay");
        assert_eq!(std::fs::read(&escape).unwrap(), b"escape");

        let image = RgbaImage::new(1, 1, vec![9, 8, 7, 255]).unwrap();
        assert!(cache.store(key, &image));
        let stored = cache.path_for(key).unwrap();
        stays_inside(&data, &stored, cache.dir());
        assert!(stored.is_file());
        assert_eq!(cache.load(key).unwrap(), image);
        assert_eq!(std::fs::read(&shelf).unwrap(), b"shelf");
    }
}
