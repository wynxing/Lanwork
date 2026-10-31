//! 搜索、应用墙和收纳共用的图标缓存。
//!
//! 内存同时不超过 128 项和 8 MiB。字节数只计 RGBA 像素，不含分配器额外开销。
//! 命中顺序是内存、磁盘、系统取图。取到后写回内存和磁盘。失败不写磁盘。
//! 磁盘目录由调用方传入，正式进程用 `%LOCALAPPDATA%\Lanwork\cache\icons`。
//! 搜索和应用墙的可见项，以及空闲时的应用墙第一屏，才会来取。本模块不决定哪一条可见。
//! 第一屏放不进这个上限时，留下列表开头的那一段；再往后的预取只写磁盘。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use super::icon_disk::{self, IconDisk, SourceStamp};
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

#[derive(Clone)]
struct MemoryIcon {
    image: RgbaImage,
    stamp: SourceStamp,
}

/// 最近最少使用。满 128 项或放进下一项会超过 8 MiB 时，丢掉最久未用的项。
///
/// 源文件的修改时间或长度变了，内存里的旧图也不再使用。
pub struct IconCache {
    loader: Arc<Mutex<Box<dyn IconLoader>>>,
    disk: Option<IconDisk>,
    map: HashMap<IconKey, MemoryIcon>,
    order: VecDeque<IconKey>,
    bytes: usize,
    gate: Arc<IconGate>,
}

impl IconCache {
    #[must_use]
    pub fn new(loader: impl IconLoader + 'static) -> Self {
        Self::build(loader, None)
    }

    /// 解码副本写到 `dir`。目录可以还不存在，第一次写盘时创建。
    #[must_use]
    pub fn with_disk(
        loader: impl IconLoader + 'static,
        dir: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self::build(loader, Some(IconDisk::new(dir)))
    }

    fn build(loader: impl IconLoader + 'static, disk: Option<IconDisk>) -> Self {
        Self {
            loader: Arc::new(Mutex::new(Box::new(loader))),
            disk,
            map: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            gate: Arc::new(IconGate::new()),
        }
    }

    /// 当前进程的壳层取图，只留在内存里。非 Windows 上取不到图。
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

    /// 命中内存或磁盘则直接返回。未命中才调用取图，成功后写入内存和磁盘。失败不写入。
    ///
    /// 这个方法在持有缓存期间调用取图，给测试用。搜索、应用墙和预热走 [`UserBatch`]
    /// 与 [`load_preload`]，取图时不占着整份缓存，预热会给用户请求让路。
    pub fn get(&mut self, key: &IconKey) -> Option<RgbaImage> {
        if let Some(image) = self.recall(key) {
            return Some(image);
        }
        let image = {
            let mut loader = guard(&self.loader);
            loader.load(key)
        }?;
        self.remember(key, image)
    }

    /// 只看内存。过期的项从内存去掉。不读磁盘，不调用系统取图。
    pub fn peek(&mut self, key: &IconKey) -> Option<RgbaImage> {
        self.fresh_memory(key)
    }

    /// 留下 `keys` 从开头起、放得进 128 项和 8 MiB 的那一段，并把它排到最近使用的一端。
    /// 已经被挤掉、磁盘上还有的，从磁盘读回。磁盘没有的跳过，不向系统取图。
    /// 这一段之后的键可以不在内存里。
    pub fn prefer(&mut self, keys: &[IconKey]) {
        let mut kept: Vec<(IconKey, Option<MemoryIcon>)> = Vec::new();
        let mut bytes = 0usize;
        for key in keys {
            if kept.len() >= ICON_CACHE_ITEMS {
                break;
            }
            let (size, revived) = if self.has_fresh(key) {
                let size = self
                    .map
                    .get(key)
                    .map(|icon| icon.image.pixels.len())
                    .unwrap_or(0);
                (size, None)
            } else if let Some(icon) = self.disk_copy(key) {
                (icon.image.pixels.len(), Some(icon))
            } else {
                continue;
            };
            if size > ICON_CACHE_BYTES {
                continue;
            }
            if bytes + size > ICON_CACHE_BYTES {
                break;
            }
            bytes += size;
            kept.push((key.clone(), revived));
        }
        if kept.iter().all(|(_, revived)| revived.is_none()) {
            for (key, _) in kept.iter().rev() {
                self.touch(key);
            }
            return;
        }
        let mut restored = Vec::with_capacity(kept.len());
        for (key, revived) in kept {
            let Some(icon) = revived.or_else(|| self.fresh_copy(&key)) else {
                continue;
            };
            restored.push((key, icon));
        }
        for (key, _) in &restored {
            self.remove_key(key);
        }
        for (key, icon) in restored.into_iter().rev() {
            self.insert(key, icon);
        }
    }

    /// 内存，然后磁盘。磁盘命中写回内存。系统取图不在这里。
    fn recall(&mut self, key: &IconKey) -> Option<RgbaImage> {
        if let Some(image) = self.fresh_memory(key) {
            return Some(image);
        }
        let (image, stamp) = self.disk.as_ref().and_then(|disk| disk.load(key))?;
        self.insert(
            key.clone(),
            MemoryIcon {
                image: image.clone(),
                stamp,
            },
        );
        if self.map.contains_key(key) {
            self.map.get(key).map(|entry| entry.image.clone())
        } else {
            Some(image)
        }
    }

    fn fresh_memory(&mut self, key: &IconKey) -> Option<RgbaImage> {
        let image = self.fresh_copy(key).map(|icon| icon.image);
        if image.is_some() {
            self.touch(key);
        }
        image
    }

    /// 内存里的图还对得上源文件。对不上就从内存去掉。不改使用次序。
    fn has_fresh(&mut self, key: &IconKey) -> bool {
        let stamp = icon_disk::source_stamp(&key.path);
        match self.map.get(key) {
            Some(entry) if entry.stamp == stamp => true,
            Some(_) => {
                self.remove_key(key);
                false
            }
            None => false,
        }
    }

    /// 内存里的图还对得上源文件时复制出来。对不上就从内存去掉。不改使用次序。
    fn fresh_copy(&mut self, key: &IconKey) -> Option<MemoryIcon> {
        if self.has_fresh(key) {
            self.map.get(key).cloned()
        } else {
            None
        }
    }

    fn disk_copy(&self, key: &IconKey) -> Option<MemoryIcon> {
        let (image, stamp) = self.disk.as_ref().and_then(|disk| disk.load(key))?;
        Some(MemoryIcon { image, stamp })
    }

    fn remember(&mut self, key: &IconKey, image: RgbaImage) -> Option<RgbaImage> {
        let stamp = icon_disk::source_stamp(&key.path);
        self.insert(
            key.clone(),
            MemoryIcon {
                image: image.clone(),
                stamp,
            },
        );
        if !self.map.contains_key(key) {
            return None;
        }
        if let Some(disk) = &self.disk {
            disk.store(key, &image, stamp);
        }
        Some(image)
    }

    fn touch(&mut self, key: &IconKey) {
        if let Some(index) = self.order.iter().position(|item| item == key)
            && let Some(item) = self.order.remove(index)
        {
            self.order.push_back(item);
        }
    }

    fn insert(&mut self, key: IconKey, icon: MemoryIcon) {
        let incoming = icon.image.pixels.len();
        if incoming > ICON_CACHE_BYTES {
            return;
        }
        if self.map.contains_key(&key) {
            self.remove_key(&key);
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
        self.map.insert(key, icon);
    }

    fn evict_oldest(&mut self) {
        let Some(key) = self.order.pop_front() else {
            return;
        };
        if let Some(icon) = self.map.remove(&key) {
            self.bytes = self.bytes.saturating_sub(icon.image.pixels.len());
        }
    }

    fn remove_key(&mut self, key: &IconKey) {
        if let Some(icon) = self.map.remove(key) {
            self.bytes = self.bytes.saturating_sub(icon.image.pixels.len());
        }
        if let Some(index) = self.order.iter().position(|item| item == key) {
            self.order.remove(index);
        }
    }
}

struct IconGate {
    user: Mutex<usize>,
    cv: Condvar,
}

impl IconGate {
    fn new() -> Self {
        Self {
            user: Mutex::new(0),
            cv: Condvar::new(),
        }
    }

    fn enter_user(self: &Arc<Self>) -> UserHold {
        let mut user = guard(&self.user);
        *user = user.saturating_add(1);
        UserHold {
            gate: Arc::clone(self),
        }
    }

    fn wait_idle(&self) {
        let mut user = guard(&self.user);
        while *user > 0 {
            user = self.cv.wait(user).unwrap_or_else(|err| err.into_inner());
        }
    }

    fn is_idle(&self) -> bool {
        *guard(&self.user) == 0
    }
}

struct UserHold {
    gate: Arc<IconGate>,
}

impl Drop for UserHold {
    fn drop(&mut self) {
        let mut user = guard(&self.gate.user);
        *user = user.saturating_sub(1);
        if *user == 0 {
            self.gate.cv.notify_all();
        }
    }
}

/// 一次用户取图。持有期间预热不会开始新的系统取图。
pub struct UserBatch<'a> {
    cache: &'a Mutex<IconCache>,
    _hold: UserHold,
}

impl<'a> UserBatch<'a> {
    #[must_use]
    pub fn enter(cache: &'a Mutex<IconCache>) -> Self {
        let gate = {
            let cache = guard(cache);
            Arc::clone(&cache.gate)
        };
        Self {
            cache,
            _hold: gate.enter_user(),
        }
    }

    pub fn load(&self, key: &IconKey) -> Option<RgbaImage> {
        match fetch(self.cache, key, false) {
            Fetch::Image(image) => image,
            Fetch::Yield => None,
        }
    }
}

/// 预热一次取图的结果。`Yield` 表示用户请求已经在取，这次没有向系统要图。
#[derive(Debug)]
pub enum PreloadLoad {
    Ready(Option<RgbaImage>),
    Yield,
}

/// 空闲预热的一次取图。用户批次还在时等到它结束；等到之后用户又开始了，就返回 [`PreloadLoad::Yield`]。
/// 成功的图写入内存和磁盘。
pub fn load_preload(cache: &Mutex<IconCache>, key: &IconKey) -> PreloadLoad {
    wait_until_idle(cache);
    match fetch(cache, key, true) {
        Fetch::Image(image) => PreloadLoad::Ready(image),
        Fetch::Yield => PreloadLoad::Yield,
    }
}

/// 空闲预热里、内存放不下的那一段。写入磁盘，不放进内存，也不改已在内存里的次序。
/// 用户批次还在时等到它结束；等到之后用户又开始了，就返回 [`PreloadLoad::Yield`]。
pub fn load_preload_disk(cache: &Mutex<IconCache>, key: &IconKey) -> PreloadLoad {
    wait_until_idle(cache);
    match fetch_disk(cache, key) {
        Fetch::Image(image) => PreloadLoad::Ready(image),
        Fetch::Yield => PreloadLoad::Yield,
    }
}

fn wait_until_idle(cache: &Mutex<IconCache>) {
    let gate = {
        let cache = guard(cache);
        Arc::clone(&cache.gate)
    };
    gate.wait_idle();
}

enum Fetch {
    Image(Option<RgbaImage>),
    Yield,
}

fn fetch(cache: &Mutex<IconCache>, key: &IconKey, preload: bool) -> Fetch {
    let (loader, gate) = {
        let mut cache = guard(cache);
        if let Some(image) = cache.recall(key) {
            return Fetch::Image(Some(image));
        }
        if preload && !cache.gate.is_idle() {
            return Fetch::Yield;
        }
        (Arc::clone(&cache.loader), Arc::clone(&cache.gate))
    };
    let image = {
        let mut loader = guard(&loader);
        if preload && !gate.is_idle() {
            return Fetch::Yield;
        }
        loader.load(key)
    };
    let Some(image) = image else {
        return Fetch::Image(None);
    };
    let mut cache = guard(cache);
    if let Some(existing) = cache.recall(key) {
        return Fetch::Image(Some(existing));
    }
    Fetch::Image(cache.remember(key, image))
}

/// 预热后段：命中内存或磁盘时不改内存。未命中才向系统取图，成功后只写磁盘。
fn fetch_disk(cache: &Mutex<IconCache>, key: &IconKey) -> Fetch {
    let (loader, gate) = {
        let mut cache = guard(cache);
        if let Some(icon) = cache.fresh_copy(key) {
            return Fetch::Image(Some(icon.image));
        }
        if !cache.gate.is_idle() {
            return Fetch::Yield;
        }
        if let Some(icon) = cache.disk_copy(key) {
            return Fetch::Image(Some(icon.image));
        }
        (Arc::clone(&cache.loader), Arc::clone(&cache.gate))
    };
    let image = {
        let mut loader = guard(&loader);
        if !gate.is_idle() {
            return Fetch::Yield;
        }
        loader.load(key)
    };
    let Some(image) = image else {
        return Fetch::Image(None);
    };
    let cache = guard(cache);
    if let Some(disk) = &cache.disk {
        let stamp = icon_disk::source_stamp(&key.path);
        disk.store(key, &image, stamp);
    }
    Fetch::Image(Some(image))
}

fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
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
        // SAFETY: 参数是空的屏幕 DC。返回的句柄只交给下面的 Drop；失败时句柄无效。
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
    use std::sync::{Condvar, Mutex};
    use std::time::Duration;

    use super::{
        ICON_CACHE_BYTES, ICON_CACHE_ITEMS, IconCache, IconLoader, PreloadLoad, RgbaImage,
        UserBatch, load_preload, load_preload_disk,
    };
    use crate::dispatch::model::IconKey;
    use crate::storage::test_temp::TempDir;

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

    fn fixed_disk(dir: &std::path::Path, image: RgbaImage) -> (IconCache, Arc<AtomicUsize>) {
        let loads = Arc::new(AtomicUsize::new(0));
        let cache = IconCache::with_disk(
            Fixed {
                image,
                loads: Arc::clone(&loads),
            },
            dir,
        );
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

    #[test]
    fn prefer_keeps_the_front_of_a_full_cache() {
        let (mut cache, _) = fixed(pixel());
        let count = ICON_CACHE_ITEMS as i32;
        for index in 0..count {
            assert!(cache.get(&key(index)).is_some());
        }
        let preferred: Vec<IconKey> = (0..10).map(key).collect();
        cache.prefer(&preferred);
        assert!(cache.get(&key(count)).is_some());
        assert!(cache.contains(&key(0)));
        assert!(cache.contains(&key(9)));
        assert!(!cache.contains(&key(10)));
        assert!(cache.contains(&key(count)));
    }

    #[test]
    fn prefer_puts_the_front_back_when_the_list_outruns_the_cache() {
        let temp = TempDir::new();
        let (mut cache, loads) = fixed_disk(temp.path(), pixel());
        let count = ICON_CACHE_ITEMS + 40;
        let keys: Vec<IconKey> = (0..count as i32).map(key).collect();
        for icon in &keys {
            assert!(cache.get(icon).is_some());
        }
        assert!(!cache.contains(&key(0)));
        assert!(cache.contains(keys.last().expect("tail")));
        let extracted = loads.load(Ordering::SeqCst);
        cache.prefer(&keys);
        assert_eq!(loads.load(Ordering::SeqCst), extracted);
        assert!(cache.contains(&key(0)));
        assert!(cache.contains(&key(ICON_CACHE_ITEMS as i32 - 1)));
        assert!(!cache.contains(keys.last().expect("tail")));
        assert!(cache.len() <= ICON_CACHE_ITEMS);
        assert!(cache.byte_len() <= ICON_CACHE_BYTES);
    }

    #[test]
    fn preload_past_capacity_keeps_the_front_and_leaves_the_tail_on_disk() {
        let temp = TempDir::new();
        let dir = temp.path().join("icons");
        let loads = Arc::new(AtomicUsize::new(0));
        let cache = Arc::new(Mutex::new(IconCache::with_disk(
            Fixed {
                image: pixel(),
                loads: Arc::clone(&loads),
            },
            &dir,
        )));
        for index in 0..ICON_CACHE_ITEMS as i32 {
            assert!(matches!(
                load_preload(&cache, &key(index)),
                PreloadLoad::Ready(Some(_))
            ));
        }
        let tail = key(ICON_CACHE_ITEMS as i32);
        assert!(matches!(
            load_preload_disk(&cache, &tail),
            PreloadLoad::Ready(Some(_))
        ));
        {
            let cache = cache.lock().unwrap();
            assert!(cache.contains(&key(0)));
            assert!(!cache.contains(&tail));
            assert_eq!(cache.len(), ICON_CACHE_ITEMS);
        }
        let extracted = loads.load(Ordering::SeqCst);
        let (mut again, again_loads) = fixed_disk(&dir, pixel());
        assert!(again.get(&tail).is_some());
        assert_eq!(again_loads.load(Ordering::SeqCst), 0);
        assert_eq!(loads.load(Ordering::SeqCst), extracted);
    }

    struct Marked {
        loads: Arc<AtomicUsize>,
        fail: bool,
    }

    impl IconLoader for Marked {
        fn load(&mut self, key: &IconKey) -> Option<RgbaImage> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return None;
            }
            let len = std::fs::metadata(&key.path)
                .map(|meta| meta.len())
                .unwrap_or(0);
            let mark = u8::try_from(key.index).unwrap_or(0) ^ u8::try_from(len).unwrap_or(255);
            RgbaImage::new(1, 1, vec![mark, 2, 3, 255])
        }
    }

    fn marked(dir: &std::path::Path, fail: bool) -> (IconCache, Arc<AtomicUsize>) {
        let loads = Arc::new(AtomicUsize::new(0));
        let cache = IconCache::with_disk(
            Marked {
                loads: Arc::clone(&loads),
                fail,
            },
            dir,
        );
        (cache, loads)
    }

    fn sample_file(dir: &TempDir) -> IconKey {
        let file = dir.path().join("app.exe");
        std::fs::write(&file, b"v1").unwrap();
        crate::dispatch::icon_disk::set_modified(&file, 1_700_000_000);
        IconKey {
            path: file.display().to_string(),
            index: 1,
        }
    }

    #[test]
    fn disk_hit_does_not_load_again_and_index_is_part_of_the_key() {
        let temp = TempDir::new();
        let icon = sample_file(&temp);
        let dir = temp.path().join("icons");
        let (mut cache, loads) = marked(&dir, false);
        let first = cache.get(&icon).expect("extracted");
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        let other = IconKey {
            path: icon.path.clone(),
            index: icon.index + 1,
        };
        let second = cache.get(&other).expect("other index");
        assert_ne!(first, second);
        assert_eq!(loads.load(Ordering::SeqCst), 2);

        let (mut again, loads) = marked(&dir, false);
        assert_eq!(again.get(&icon), Some(first));
        assert_eq!(again.get(&other), Some(second));
        assert_eq!(loads.load(Ordering::SeqCst), 0);
        assert!(again.peek(&icon).is_some());
    }

    #[test]
    fn a_changed_source_file_is_extracted_again() {
        let temp = TempDir::new();
        let icon = sample_file(&temp);
        let dir = temp.path().join("icons");
        let (mut cache, loads) = marked(&dir, false);
        let first = cache.get(&icon).expect("first");
        assert_eq!(loads.load(Ordering::SeqCst), 1);

        std::fs::write(&icon.path, b"v2-longer").unwrap();
        crate::dispatch::icon_disk::set_modified(std::path::Path::new(&icon.path), 1_700_000_100);
        let (mut again, loads) = marked(&dir, false);
        let second = again.get(&icon).expect("refreshed");
        assert_ne!(second, first);
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        assert!(again.peek(&icon).is_some());

        let (mut failing, loads) = marked(&dir, true);
        std::fs::write(&icon.path, b"v3").unwrap();
        crate::dispatch::icon_disk::set_modified(std::path::Path::new(&icon.path), 1_700_000_200);
        assert!(failing.get(&icon).is_none());
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        let (mut still, loads) = marked(&dir, true);
        assert!(still.get(&icon).is_none());
        assert_eq!(
            loads.load(Ordering::SeqCst),
            1,
            "a failed extract was kept on disk"
        );
    }

    #[test]
    fn a_failed_extract_is_not_written() {
        let temp = TempDir::new();
        let icon = sample_file(&temp);
        let dir = temp.path().join("icons");
        let (mut cache, loads) = marked(&dir, true);
        assert!(cache.get(&icon).is_none());
        assert!(cache.get(&icon).is_none());
        assert_eq!(loads.load(Ordering::SeqCst), 2);
        assert!(
            !dir.exists() || std::fs::read_dir(&dir).unwrap().next().is_none(),
            "a failed extract left a file"
        );
    }

    struct Slow {
        state: Arc<SlowState>,
    }

    struct SlowState {
        started: Mutex<Vec<String>>,
        started_cv: Condvar,
        release_slow: Mutex<bool>,
        release_cv: Condvar,
    }

    impl IconLoader for Slow {
        fn load(&mut self, key: &IconKey) -> Option<RgbaImage> {
            {
                let mut started = self.state.started.lock().unwrap();
                started.push(key.path.clone());
                self.state.started_cv.notify_all();
            }
            if key.path == "slow" {
                let mut release = self.state.release_slow.lock().unwrap();
                while !*release {
                    release = self.state.release_cv.wait(release).unwrap();
                }
            }
            Some(pixel())
        }
    }

    fn wait_started(state: &SlowState, count: usize) {
        let mut started = state.started.lock().unwrap();
        let mut spins = 0;
        while started.len() < count {
            let (guard, timeout) = state
                .started_cv
                .wait_timeout(started, Duration::from_secs(2))
                .unwrap();
            started = guard;
            if timeout.timed_out() {
                spins += 1;
                if spins > 1 {
                    break;
                }
            }
        }
    }

    fn user_batch_runs_before(next: fn(&Mutex<IconCache>, &IconKey) -> PreloadLoad) {
        let state = Arc::new(SlowState {
            started: Mutex::new(Vec::new()),
            started_cv: Condvar::new(),
            release_slow: Mutex::new(false),
            release_cv: Condvar::new(),
        });
        let cache = Arc::new(Mutex::new(IconCache::new(Slow {
            state: Arc::clone(&state),
        })));

        let slow_cache = Arc::clone(&cache);
        let slow = std::thread::spawn(move || {
            load_preload(
                &slow_cache,
                &IconKey {
                    path: "slow".to_owned(),
                    index: 0,
                },
            )
        });
        wait_started(&state, 1);

        let user_cache = Arc::clone(&cache);
        let entered = Arc::new((Mutex::new(false), Condvar::new()));
        let entered_flag = Arc::clone(&entered);
        let user = std::thread::spawn(move || {
            let batch = UserBatch::enter(&user_cache);
            {
                let mut flag = entered_flag.0.lock().unwrap();
                *flag = true;
                entered_flag.1.notify_all();
            }
            batch.load(&IconKey {
                path: "user".to_owned(),
                index: 0,
            })
        });
        {
            let mut flag = entered.0.lock().unwrap();
            while !*flag {
                flag = entered.1.wait(flag).unwrap();
            }
        }

        let next_cache = Arc::clone(&cache);
        let next = std::thread::spawn(move || {
            next(
                &next_cache,
                &IconKey {
                    path: "next".to_owned(),
                    index: 0,
                },
            )
        });

        let deadline = std::time::Instant::now() + Duration::from_millis(150);
        loop {
            let started = state.started.lock().unwrap();
            assert_eq!(
                started.as_slice(),
                ["slow"],
                "preload or the user batch ran ahead of the in-flight icon"
            );
            let now = std::time::Instant::now();
            if now >= deadline {
                break;
            }
            let (_started, timeout) = state
                .started_cv
                .wait_timeout(started, deadline.saturating_duration_since(now))
                .unwrap();
            if timeout.timed_out() {
                break;
            }
        }

        {
            let mut release = state.release_slow.lock().unwrap();
            *release = true;
            state.release_cv.notify_all();
        }
        assert!(matches!(slow.join().unwrap(), PreloadLoad::Ready(Some(_))));
        assert!(user.join().unwrap().is_some());
        assert!(matches!(next.join().unwrap(), PreloadLoad::Ready(Some(_))));
        let started = state.started.lock().unwrap();
        assert_eq!(started.as_slice(), ["slow", "user", "next"]);
    }

    #[test]
    fn preload_waits_until_the_user_batch_finishes() {
        user_batch_runs_before(load_preload);
    }

    #[test]
    fn preload_disk_waits_until_the_user_batch_finishes() {
        user_batch_runs_before(load_preload_disk);
    }
}
