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

/// 解开 PNG、ICO、JPEG、GIF、BMP。SVG 和坏数据返回 `None`。过大的图先缩到 256。
pub(crate) fn decode(bytes: &[u8], content_type: Option<&str>) -> Option<RgbaImage> {
    if bytes.is_empty() || bytes.len() > WEB_ICON_BYTES_LIMIT || looks_like_svg(bytes, content_type)
    {
        return None;
    }
    let _com = ComGuard::enter();
    decode_wic(bytes)
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

    /// 同一张 PNG 包在 ICO 里。
    const ICO: &[u8] = &[
        0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x01, 0x01, 0x00, 0x00, 0x01, 0x00, 0x20, 0x00, 0x45,
        0x00, 0x00, 0x00, 0x16, 0x00, 0x00, 0x00, 0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A,
        0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49,
        0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00,
        0xC9, 0xFE, 0x92, 0xEF, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60,
        0x82,
    ];

    #[test]
    fn png_and_ico_decode_and_garbage_stays_empty() {
        let png = decode(PNG, Some("image/png")).expect("png");
        assert_eq!((png.width, png.height), (1, 1));
        assert_eq!(png.pixels, [255, 0, 0, 255]);
        let ico = decode(ICO, Some("image/x-icon")).expect("ico");
        assert_eq!((ico.width, ico.height), (1, 1));
        assert_eq!(ico.pixels, [255, 0, 0, 255]);
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
