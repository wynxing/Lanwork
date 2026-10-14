//! 用 Windows Imaging Component 把网站图标解成 RGBA。
//!
//! SVG 解不开，交给调用方换下一条。失败时返回 `None`。

use lanwork_core::dispatch::RgbaImage;
use lanwork_core::shelves::{WEB_ICON_BYTES_LIMIT, WEB_ICON_MAX_EDGE, fit_icon_edge};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICBitmapScaler, IWICFormatConverter,
    IWICImagingFactory, WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant,
    WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnLoad,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};

/// 这一线程上的 COM。`S_OK` 和 `S_FALSE` 都要在结束时 `CoUninitialize`。
pub(crate) struct ComGuard {
    uninit: bool,
}

impl ComGuard {
    pub(crate) fn enter() -> Self {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        Self { uninit: hr.is_ok() }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.uninit {
            unsafe { CoUninitialize() }
        }
    }
}

const PNG_SIG: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// 解开 PNG、ICO、JPEG、GIF、BMP。SVG 和坏数据返回 `None`。过大的图先缩到 256。
///
/// PNG 形式的 ICO 目录项色面数和位深是 0。有的 favicon.ico 写成 1 和 32，WIC 会解不开。
/// 那种目录项先改成 0 再解。整份仍解不开时，把里面的 PNG 抽出来再解。
pub(crate) fn decode(bytes: &[u8], content_type: Option<&str>) -> Option<RgbaImage> {
    if bytes.is_empty() || bytes.len() > WEB_ICON_BYTES_LIMIT || looks_like_svg(bytes, content_type)
    {
        return None;
    }
    let _com = ComGuard::enter();
    if let Some(mut canonical) = png_ico_zero_planes(bytes)
        && let Some(image) = decode_owned(&mut canonical)
    {
        return Some(image);
    }
    decode_wic(bytes).or_else(|| decode_png_in_ico(bytes))
}

/// 只改 PNG 载荷的目录项。BMP 形式的色面数和位深不动。没有要改的项时返回 `None`。
fn png_ico_zero_planes(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 22 || bytes[0] != 0 || bytes[1] != 0 || bytes[2] != 1 || bytes[3] != 0 {
        return None;
    }
    let count = usize::from(u16::from_le_bytes([bytes[4], bytes[5]])).min(64);
    if count == 0 {
        return None;
    }
    let mut out = None;
    for index in 0..count {
        let at = 6 + index * 16;
        let Some(entry) = bytes.get(at..at + 16) else {
            break;
        };
        let Ok(size) = usize::try_from(u32::from_le_bytes([
            entry[8], entry[9], entry[10], entry[11],
        ])) else {
            continue;
        };
        let Ok(offset) = usize::try_from(u32::from_le_bytes([
            entry[12], entry[13], entry[14], entry[15],
        ])) else {
            continue;
        };
        let Some(end) = offset.checked_add(size) else {
            continue;
        };
        let Some(payload) = bytes.get(offset..end) else {
            continue;
        };
        if !payload.starts_with(PNG_SIG) || entry[4..8] == [0, 0, 0, 0] {
            continue;
        }
        let buf = out.get_or_insert_with(|| bytes.to_vec());
        buf[at + 4] = 0;
        buf[at + 5] = 0;
        buf[at + 6] = 0;
        buf[at + 7] = 0;
    }
    out
}

/// 目录项里以 PNG 开头的图像。多张时留边长更大的那张。
fn decode_png_in_ico(bytes: &[u8]) -> Option<RgbaImage> {
    if bytes.len() < 6 || bytes[0] != 0 || bytes[1] != 0 || bytes[2] != 1 || bytes[3] != 0 {
        return None;
    }
    let count = usize::from(u16::from_le_bytes([bytes[4], bytes[5]])).min(64);
    if count == 0 {
        return None;
    }
    let mut best: Option<(u32, RgbaImage)> = None;
    for index in 0..count {
        let at = 6 + index * 16;
        let Some(entry) = bytes.get(at..at + 16) else {
            break;
        };
        let Ok(size) = usize::try_from(u32::from_le_bytes([
            entry[8], entry[9], entry[10], entry[11],
        ])) else {
            continue;
        };
        let Ok(offset) = usize::try_from(u32::from_le_bytes([
            entry[12], entry[13], entry[14], entry[15],
        ])) else {
            continue;
        };
        let Some(end) = offset.checked_add(size) else {
            continue;
        };
        let Some(payload) = bytes.get(offset..end) else {
            continue;
        };
        if !payload.starts_with(PNG_SIG) {
            continue;
        }
        let Some(image) = decode_wic(payload) else {
            continue;
        };
        let width = if entry[0] == 0 {
            256
        } else {
            u32::from(entry[0])
        };
        let height = if entry[1] == 0 {
            256
        } else {
            u32::from(entry[1])
        };
        let area = width.saturating_mul(height);
        let replace = match &best {
            Some((prev, _)) => area > *prev,
            None => true,
        };
        if replace {
            best = Some((area, image));
        }
    }
    best.map(|(_, image)| image)
}

fn looks_like_svg(bytes: &[u8], content_type: Option<&str>) -> bool {
    if content_type.is_some_and(|kind| kind.to_ascii_lowercase().contains("svg")) {
        return true;
    }
    let head = &bytes[..bytes.len().min(256)];
    let text = String::from_utf8_lossy(head).to_ascii_lowercase();
    text.contains("<svg")
}

fn decode_wic(bytes: &[u8]) -> Option<RgbaImage> {
    let mut bytes = bytes.to_vec();
    decode_owned(&mut bytes)
}

fn decode_owned(bytes: &mut [u8]) -> Option<RgbaImage> {
    unsafe { decode_com(bytes) }
}

unsafe fn decode_com(bytes: &mut [u8]) -> Option<RgbaImage> {
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let stream = factory.CreateStream().ok()?;
        stream.InitializeFromMemory(bytes).ok()?;
        let decoder = factory
            .CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let mut src_w = 0u32;
        let mut src_h = 0u32;
        frame.GetSize(&mut src_w, &mut src_h).ok()?;
        let (width, height) = fit_icon_edge(src_w, src_h)?;
        let scaler: IWICBitmapScaler = factory.CreateBitmapScaler().ok()?;
        scaler
            .Initialize(&frame, width, height, WICBitmapInterpolationModeFant)
            .ok()?;
        let converter: IWICFormatConverter = factory.CreateFormatConverter().ok()?;
        converter
            .Initialize(
                &scaler,
                &GUID_WICPixelFormat32bppRGBA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .ok()?;
        let stride = width.checked_mul(4)?;
        let len = usize::try_from(stride)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?;
        if len > WEB_ICON_MAX_EDGE as usize * WEB_ICON_MAX_EDGE as usize * 4 {
            return None;
        }
        let mut pixels = vec![0u8; len];
        converter
            .CopyPixels(std::ptr::null(), stride, &mut pixels)
            .ok()?;
        RgbaImage::new(width, height, pixels)
    }
}

#[cfg(test)]
mod tests {
    use super::decode;

    /// 1×1 红色 PNG。
    const PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8,
        0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0xC9, 0xFE, 0x92, 0xEF, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    /// PNG 形式的 ICO。色面数和位深按调用方写入。规范写法是 0 和 0。
    fn png_in_ico(planes: u16, bit_count: u16) -> Vec<u8> {
        let mut ico = Vec::with_capacity(22 + PNG.len());
        ico.extend_from_slice(&0u16.to_le_bytes());
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&[1, 1, 0, 0]);
        ico.extend_from_slice(&planes.to_le_bytes());
        ico.extend_from_slice(&bit_count.to_le_bytes());
        let size = u32::try_from(PNG.len()).expect("png len");
        ico.extend_from_slice(&size.to_le_bytes());
        ico.extend_from_slice(&22u32.to_le_bytes());
        ico.extend_from_slice(PNG);
        ico
    }

    #[test]
    fn png_and_ico_decode_and_garbage_stays_empty() {
        let png = decode(PNG, Some("image/png")).expect("png");
        assert_eq!((png.width, png.height), (1, 1));
        assert_eq!(png.pixels, [255, 0, 0, 255]);
        let canonical = png_in_ico(0, 0);
        assert_eq!(&canonical[10..14], &[0, 0, 0, 0]);
        let legacy = png_in_ico(1, 32);
        assert_eq!(&legacy[10..14], &[1, 0, 32, 0]);
        for ico in [canonical, legacy] {
            let image = decode(&ico, Some("image/x-icon")).expect("ico");
            assert_eq!((image.width, image.height), (1, 1));
            assert_eq!(image.pixels, [255, 0, 0, 255]);
        }
        assert!(decode(b"not-an-image", None).is_none());
        assert!(
            decode(
                b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
                Some("image/svg+xml")
            )
            .is_none()
        );
    }
}
