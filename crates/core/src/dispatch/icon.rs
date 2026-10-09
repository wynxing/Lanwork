//! 搜索和收纳共用的图标缓存。
//!
//! 同时不超过 128 项和 8 MiB。字节数只计 RGBA 像素，不含分配器额外开销。
//! 只应由可见项来取。本模块不决定哪一条可见。

use std::collections::{HashMap, VecDeque};

use super::model::IconKey;

/// 项数上限。
pub const ICON_CACHE_ITEMS: usize = 128;

/// 像素字节上限。
pub const ICON_CACHE_BYTES: usize = 8 * 1024 * 1024;

/// 交给 Slint 的一张图。像素是 RGBA，从左到右、从上到下。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl RgbaImage {
    #[must_use]
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Option<Self> {
        let expected = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        if expected == 0 || pixels.len() != expected {
            return None;
        }
        Some(Self {
            width,
            height,
            pixels,
        })
    }
}

/// 按路径取一张 RGBA 图。失败时返回 `None`，缓存不记下这次失败。
pub trait IconLoader: Send {
    fn load(&mut self, key: &IconKey) -> Option<RgbaImage>;
}

/// 什么都不取到。测试和还没有壳层图标的构建用它。
#[derive(Debug, Default)]
pub struct MissingIcons;

impl IconLoader for MissingIcons {
    fn load(&mut self, _key: &IconKey) -> Option<RgbaImage> {
        None
    }
}

/// 当前进程的壳层取图。非 Windows 上取不到图。
#[derive(Debug, Default)]
pub struct ShellIcons;

impl IconLoader for ShellIcons {
    fn load(&mut self, key: &IconKey) -> Option<RgbaImage> {
        load_shell_icon(key)
    }
}

/// 最近最少使用。满 128 项或放进下一项会超过 8 MiB 时，丢掉最久未用的项。
pub struct IconCache {
    loader: Box<dyn IconLoader>,
    map: HashMap<IconKey, RgbaImage>,
    order: VecDeque<IconKey>,
    bytes: usize,
}

impl IconCache {
    #[must_use]
    pub fn new(loader: impl IconLoader + 'static) -> Self {
        Self {
            loader: Box::new(loader),
            map: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
        }
    }

    /// 当前进程的壳层取图。非 Windows 上取不到图。
    #[must_use]
    pub fn shell() -> Self {
        Self::new(ShellIcons)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.order.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.bytes
    }

    #[must_use]
    pub fn contains(&self, key: &IconKey) -> bool {
        self.map.contains_key(key)
    }

    /// 命中则把这一项记成刚用过。未命中才调用取图。
    pub fn get(&mut self, key: &IconKey) -> Option<RgbaImage> {
        if self.map.contains_key(key) {
            self.touch(key);
            return self.map.get(key).cloned();
        }
        let image = self.loader.load(key)?;
        self.insert(key.clone(), image);
        self.map.get(key).cloned()
    }

    fn touch(&mut self, key: &IconKey) {
        if let Some(index) = self.order.iter().position(|item| item == key)
            && let Some(item) = self.order.remove(index)
        {
            self.order.push_back(item);
        }
    }

    fn insert(&mut self, key: IconKey, image: RgbaImage) {
        let incoming = image.pixels.len();
        if incoming > ICON_CACHE_BYTES {
            return;
        }
        while !self.order.is_empty()
            && (self.order.len() >= ICON_CACHE_ITEMS || self.bytes + incoming > ICON_CACHE_BYTES)
        {
            self.evict_oldest();
        }
        if self.order.len() >= ICON_CACHE_ITEMS || self.bytes + incoming > ICON_CACHE_BYTES {
            return;
        }
        self.bytes += incoming;
        self.order.push_back(key.clone());
        self.map.insert(key, image);
    }

    fn evict_oldest(&mut self) {
        let Some(key) = self.order.pop_front() else {
            return;
        };
        if let Some(image) = self.map.remove(&key) {
            self.bytes = self.bytes.saturating_sub(image.pixels.len());
        }
    }
}

#[cfg(windows)]
fn load_shell_icon(key: &IconKey) -> Option<RgbaImage> {
    shell::load(key)
}

#[cfg(not(windows))]
fn load_shell_icon(_key: &IconKey) -> Option<RgbaImage> {
    None
}

#[cfg(windows)]
mod shell {
    use std::cell::Cell;

    use windows::Win32::Foundation::{RPC_E_CHANGED_MODE, SIZE};
    use windows::Win32::Graphics::Gdi::{
        BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
        DeleteObject, GetDIBits, GetObjectW, HDC, HGDIOBJ,
    };
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
    use windows::Win32::UI::Shell::{
        IShellItem, IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF, SIIGBF_ICONONLY,
        SIIGBF_RESIZETOFIT,
    };
    use windows::core::Interface;

    use super::RgbaImage;
    use crate::dispatch::IconKey;

    /// 请求的边长。规格没有写像素尺寸。
    const ICON_EDGE_PX: i32 = 32;
    const MAX_EDGE_PX: i32 = 256;

    thread_local! {
        static COM_READY: Cell<bool> = const { Cell::new(false) };
    }

    pub(super) fn load(key: &IconKey) -> Option<RgbaImage> {
        if key.path.is_empty() {
            return None;
        }
        ensure_com()?;
        let wide = wide_path(&key.path);
        // SAFETY: 宽路径在调用期间有效，bind context 为空。失败时不保留接口。
        let item: IShellItem = unsafe {
            SHCreateItemFromParsingName(windows::core::PCWSTR::from_raw(wide.as_ptr()), None)
        }
        .ok()?;
        let factory: IShellItemImageFactory = item.cast().ok()?;
        // SAFETY: 只请求图标，不创建窗口。失败时没有位图需要释放。
        let bitmap = unsafe {
            factory.GetImage(
                SIZE {
                    cx: ICON_EDGE_PX,
                    cy: ICON_EDGE_PX,
                },
                SIIGBF(SIIGBF_ICONONLY.0 | SIIGBF_RESIZETOFIT.0),
            )
        }
        .ok()?;
        let bitmap = Bitmap(bitmap);
        let (width, height) = bitmap_size(bitmap.0)?;
        let dc = Dc(unsafe { CreateCompatibleDC(None) });
        if dc.0.is_invalid() {
            return None;
        }
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: 0,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [windows::Win32::Graphics::Gdi::RGBQUAD::default()],
        };
        let byte_len = (width as usize)
            .checked_mul(height as usize)?
            .checked_mul(4)?;
        let mut pixels = vec![0u8; byte_len];
        // SAFETY: 像素缓冲区和 BITMAPINFO 在调用期间有效。位图和 DC 由上面的守卫持有。
        let rows = unsafe {
            GetDIBits(
                dc.0,
                bitmap.0,
                0,
                height as u32,
                Some(pixels.as_mut_ptr().cast()),
                &mut info,
                DIB_RGB_COLORS,
            )
        };
        if rows == 0 {
            return None;
        }
        bgra_to_rgba(&mut pixels);
        RgbaImage::new(width as u32, height as u32, pixels)
    }

    fn ensure_com() -> Option<()> {
        let mut ok = true;
        COM_READY.with(|ready| {
            if ready.get() {
                return;
            }
            // SAFETY: 保留参数是空的。本线程之后不再 CoUninitialize，避免和已有套间配平错。
            let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            if hr.is_err() && hr != RPC_E_CHANGED_MODE {
                ok = false;
                return;
            }
            ready.set(true);
        });
        ok.then_some(())
    }

    fn bitmap_size(bitmap: windows::Win32::Graphics::Gdi::HBITMAP) -> Option<(i32, i32)> {
        let mut info = BITMAP::default();
        // SAFETY: info 是 BITMAP，长度与 c 一致。句柄是 GetImage 刚返回的位图。
        let wrote = unsafe {
            GetObjectW(
                HGDIOBJ::from(bitmap),
                std::mem::size_of::<BITMAP>() as i32,
                Some(std::ptr::addr_of_mut!(info).cast()),
            )
        };
        if wrote == 0 {
            return None;
        }
        let width = info.bmWidth;
        let height = info.bmHeight.abs();
        if width <= 0 || height <= 0 || width > MAX_EDGE_PX || height > MAX_EDGE_PX {
            return None;
        }
        Some((width, height))
    }

    fn bgra_to_rgba(pixels: &mut [u8]) {
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }
        let has_alpha = pixels.as_chunks::<4>().0.iter().any(|pixel| pixel[3] != 0);
        if !has_alpha {
            for pixel in pixels.as_chunks_mut::<4>().0 {
                pixel[3] = 255;
            }
        }
    }

    fn wide_path(path: &str) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        std::ffi::OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    struct Bitmap(windows::Win32::Graphics::Gdi::HBITMAP);

    impl Drop for Bitmap {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                // SAFETY: 位图由 GetImage 创建，这里释放一次。
                let _ = unsafe { DeleteObject(HGDIOBJ::from(self.0)) };
            }
        }
    }

    struct Dc(HDC);

    impl Drop for Dc {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                // SAFETY: 这个 DC 由 CreateCompatibleDC 创建，只释放一次。
                let _ = unsafe { DeleteDC(self.0) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::{ICON_CACHE_BYTES, ICON_CACHE_ITEMS, IconCache, IconLoader, RgbaImage};
    use crate::dispatch::model::IconKey;

    fn key(index: i32) -> IconKey {
        IconKey {
            path: format!("item-{index}"),
            index,
        }
    }

    fn pixel() -> RgbaImage {
        RgbaImage::new(1, 1, vec![1, 2, 3, 255]).unwrap()
    }

    fn mib() -> RgbaImage {
        let width = 512u32;
        let height = 512u32;
        RgbaImage::new(width, height, vec![9u8; (width * height * 4) as usize]).unwrap()
    }

    struct Fixed {
        image: RgbaImage,
        loads: Arc<AtomicUsize>,
    }

    impl IconLoader for Fixed {
        fn load(&mut self, _key: &IconKey) -> Option<RgbaImage> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            Some(self.image.clone())
        }
    }

    fn fixed(image: RgbaImage) -> (IconCache, Arc<AtomicUsize>) {
        let loads = Arc::new(AtomicUsize::new(0));
        let cache = IconCache::new(Fixed {
            image,
            loads: Arc::clone(&loads),
        });
        (cache, loads)
    }

    #[test]
    fn lru_drops_the_least_recently_used_at_128() {
        let (mut cache, _) = fixed(pixel());
        for index in 0..ICON_CACHE_ITEMS as i32 {
            assert!(cache.get(&key(index)).is_some());
        }
        assert_eq!(cache.len(), ICON_CACHE_ITEMS);
        assert!(cache.get(&key(0)).is_some());
        assert!(cache.get(&key(ICON_CACHE_ITEMS as i32)).is_some());
        assert!(cache.contains(&key(0)));
        assert!(!cache.contains(&key(1)));
        assert!(cache.contains(&key(ICON_CACHE_ITEMS as i32)));
        assert_eq!(cache.len(), ICON_CACHE_ITEMS);
    }

    #[test]
    fn lru_drops_the_least_recently_used_at_8_mib() {
        let (mut cache, _) = fixed(mib());
        for index in 0..8 {
            assert!(cache.get(&key(index)).is_some());
        }
        assert_eq!(cache.byte_len(), ICON_CACHE_BYTES);
        assert!(cache.get(&key(0)).is_some());
        assert!(cache.get(&key(8)).is_some());
        assert!(cache.contains(&key(0)));
        assert!(!cache.contains(&key(1)));
        assert_eq!(cache.byte_len(), ICON_CACHE_BYTES);
        assert!(cache.byte_len() <= ICON_CACHE_BYTES);
        assert!(cache.len() <= ICON_CACHE_ITEMS);
    }

    #[test]
    fn a_second_get_does_not_load_again() {
        let (mut cache, loads) = fixed(pixel());
        assert!(cache.get(&key(1)).is_some());
        assert!(cache.get(&key(1)).is_some());
        assert_eq!(cache.len(), 1);
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn an_image_larger_than_the_budget_is_not_stored() {
        let height = (ICON_CACHE_BYTES / 4) as u32 + 1;
        let image = RgbaImage::new(1, height, vec![1u8; height as usize * 4]).unwrap();
        assert!(image.pixels.len() > ICON_CACHE_BYTES);
        let (mut cache, _) = fixed(image);
        assert!(cache.get(&key(0)).is_none());
        assert!(cache.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn shell_icon_for_a_text_file_is_rgba() {
        let path = std::env::temp_dir().join(format!("lanwork-icon-{}.txt", std::process::id()));
        std::fs::write(&path, b"icon").unwrap();
        let mut cache = IconCache::shell();
        let key = IconKey {
            path: path.display().to_string(),
            index: 0,
        };
        let image = cache.get(&key).expect("shell icon");
        assert_eq!(
            image.pixels.len(),
            image.width as usize * image.height as usize * 4
        );
        assert!(image.width > 0 && image.height > 0);
        assert!(image.width <= 256 && image.height <= 256);
        let _ = std::fs::remove_file(path);
        let _ = cache.get(&IconKey {
            path: r"C:\no\such\lanwork-missing-icon.xyz".to_owned(),
            index: 0,
        });
    }
}
