//! 原生 OLE 放下。只读网址格式。不读取 `FileContents` 和 `CF_HDROP`，避免把虚拟 `.url` 写到磁盘。

use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use windows::Win32::Foundation::{
    DV_E_FORMATETC, DV_E_TYMED, E_INVALIDARG, E_NOTIMPL, HGLOBAL, OLE_E_ADVISENOTSUPPORTED,
    S_FALSE, S_OK,
};
use windows::Win32::Globalization::{MULTI_BYTE_TO_WIDE_CHAR_FLAGS, MultiByteToWideChar};
use windows::Win32::System::Com::{
    DVASPECT_CONTENT, FORMATETC, IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC,
    IEnumFORMATETC_Impl, IEnumSTATDATA, IStream, STGMEDIUM, TYMED_HGLOBAL, TYMED_ISTREAM,
};
use windows::Win32::System::DataExchange::{GetClipboardFormatNameW, RegisterClipboardFormatW};
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::{
    CF_HDROP, CF_TEXT, CF_UNICODETEXT, DROPEFFECT, IDropTarget, IDropTarget_Impl, OleInitialize,
    ReleaseStgMedium,
};
use windows::core::{BOOL, HRESULT, PCWSTR, Ref, implement};

use crate::parse::{
    self, EFFECT_COPY, EFFECT_LINK, EFFECT_MOVE, Extracted, FILE_CONTENTS_NAME, FILE_GROUP_A,
    FILE_GROUP_W, HTML_FORMAT_NAME, ListedFormat, MOZ_URL_NAME, URI_LIST_NAME, URL_A_NAME,
    URL_W_NAME, UriListFound, choose_effect, decode_utf16_text, file_names_a, file_names_w,
    format_report, parse_cf_html, parse_uri_list, title_from_url_filename,
};

const MAX_BYTES: usize = 1024 * 1024;
const CF_HDROP_ID: u16 = 15;

pub(crate) struct DropState {
    pub log: Mutex<String>,
    pub edit: AtomicIsize,
    pub log_path: PathBuf,
}

pub(crate) fn publish(state: &DropState, block: &str) {
    {
        let mut log = lock_log(&state.log);
        if !log.is_empty() {
            log.push('\n');
        }
        log.push_str(block);
        if !block.ends_with('\n') {
            log.push('\n');
        }
    }
    let text = lock_log(&state.log).clone();
    print!("{block}");
    if !block.ends_with('\n') {
        println!();
    }
    let _ = std::io::Write::flush(&mut std::io::stdout());
    if let Err(error) = std::fs::write(&state.log_path, crlf(&text)) {
        println!("urldrop: log-write-error={error}");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
    apply_edit(state, &text);
}

pub(crate) fn remember_edit(state: &DropState, edit: windows::Win32::Foundation::HWND) {
    state.edit.store(edit.0 as isize, Ordering::Release);
    unsafe {
        windows::Win32::UI::WindowsAndMessaging::SendMessageW(
            edit,
            0x00C5,
            Some(windows::Win32::Foundation::WPARAM(1_048_576)),
            None,
        );
    }
    let text = lock_log(&state.log).clone();
    apply_edit(state, &text);
}

fn apply_edit(state: &DropState, text: &str) {
    let bits = state.edit.load(Ordering::Acquire);
    if bits == 0 {
        return;
    }
    let hwnd = windows::Win32::Foundation::HWND(bits as *mut std::ffi::c_void);
    let wide = utf16(text);
    unsafe {
        let _ =
            windows::Win32::UI::WindowsAndMessaging::SetWindowTextW(hwnd, PCWSTR(wide.as_ptr()));
    }
}

fn lock_log(log: &Mutex<String>) -> std::sync::MutexGuard<'_, String> {
    log.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

pub(crate) fn ensure_ole() -> Result<(), String> {
    unsafe { OleInitialize(None) }.map_err(|error| error.to_string())
}

#[implement(IDropTarget)]
struct UrlDrop {
    state: Arc<DropState>,
}

pub(crate) fn drop_target(state: Arc<DropState>) -> IDropTarget {
    UrlDrop { state }.into()
}

fn offered_and_choose(slot: *mut DROPEFFECT) -> (u32, u32) {
    let offered = if slot.is_null() {
        0
    } else {
        unsafe { (*slot).0 }
    };
    let chosen = choose_effect(offered);
    if !slot.is_null() {
        unsafe { *slot = DROPEFFECT(chosen) };
    }
    (offered, chosen)
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for UrlDrop_Impl {
    fn DragEnter(
        &self,
        _data: Ref<IDataObject>,
        _keys: windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS,
        _pt: &windows::Win32::Foundation::POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let _ = offered_and_choose(effect);
        Ok(())
    }

    fn DragOver(
        &self,
        _keys: windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS,
        _pt: &windows::Win32::Foundation::POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let _ = offered_and_choose(effect);
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        _keys: windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS,
        _pt: &windows::Win32::Foundation::POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let (offered, chosen) = offered_and_choose(effect);
        let mut extracted = match data.as_ref() {
            Some(data) => read_data_object(data),
            None => Extracted::default(),
        };
        extracted.offered_effects = offered;
        extracted.chosen_effect = chosen;
        let when = now_utc();
        publish(&self.state, &format_report("drop", &when, &extracted));
        Ok(())
    }
}

fn now_utc() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    parse::format_unix_utc(secs)
}

fn read_data_object(data: &IDataObject) -> Extracted {
    let mut extracted = Extracted::default();
    match enum_formats(data) {
        Ok(formats) => extracted.formats = formats,
        Err(error) => extracted.enum_error = Some(error),
    }
    probe_known(data, &mut extracted.formats);
    extracted.file_contents_offered = extracted
        .formats
        .iter()
        .any(|format| format.name.eq_ignore_ascii_case(FILE_CONTENTS_NAME))
        || query_available(data, registered(FILE_CONTENTS_NAME).unwrap_or(0));
    extracted.hdrop_offered = extracted
        .formats
        .iter()
        .any(|format| format.id == CF_HDROP_ID)
        || query_available(data, CF_HDROP_ID);

    if let Some(id) =
        find_name(&extracted.formats, HTML_FORMAT_NAME).or_else(|| registered(HTML_FORMAT_NAME))
    {
        match read_allowed(data, id, HTML_FORMAT_NAME) {
            Ok(Some((bytes, truncated))) => {
                extracted.html = Some(parse_cf_html(&bytes));
                extracted.html_truncated = truncated;
            }
            Ok(None) => {}
            Err(error) => extracted.html_error = Some(error),
        }
    }
    let uri_ids = uri_list_ids(data, &extracted.formats);
    for (id, name) in uri_ids {
        match read_allowed(data, id, &name) {
            Ok(Some((bytes, truncated))) => {
                let mut urls = parse_uri_list(&bytes);
                if truncated {
                    urls.push("(truncated)".to_string());
                }
                extracted.uri_lists.push(UriListFound {
                    format_name: name,
                    format_id: id,
                    urls,
                });
            }
            Ok(None) => {}
            Err(error) => extracted.uri_list_error = Some(error),
        }
    }
    let unicode_id = find_name(&extracted.formats, "CF_UNICODETEXT").or(Some(CF_UNICODETEXT.0));
    let ansi_id = find_name(&extracted.formats, "CF_TEXT").or(Some(CF_TEXT.0));
    let url_w_id = find_name(&extracted.formats, URL_W_NAME).or_else(|| registered(URL_W_NAME));
    let url_a_id = find_name(&extracted.formats, URL_A_NAME).or_else(|| registered(URL_A_NAME));
    let moz_id = find_name(&extracted.formats, MOZ_URL_NAME).or_else(|| registered(MOZ_URL_NAME));
    read_text(data, &mut extracted, unicode_id, "CF_UNICODETEXT", true);
    read_text(data, &mut extracted, ansi_id, "CF_TEXT", false);
    read_text(data, &mut extracted, url_w_id, URL_W_NAME, true);
    read_text(data, &mut extracted, url_a_id, URL_A_NAME, false);
    read_text(data, &mut extracted, moz_id, MOZ_URL_NAME, true);
    let formats = extracted.formats.clone();
    read_descriptors(data, &formats, &mut extracted);
    extracted
}

fn read_text(
    data: &IDataObject,
    extracted: &mut Extracted,
    id: Option<u16>,
    name: &str,
    wide: bool,
) {
    let Some(id) = id else {
        return;
    };
    match read_allowed(data, id, name) {
        Ok(Some((bytes, _))) => {
            let text = if wide {
                decode_utf16_text(&bytes)
            } else {
                decode_acp(&bytes)
            };
            match name {
                "CF_UNICODETEXT" => extracted.unicode_text = Some(text),
                "CF_TEXT" => extracted.ansi_text = Some(text),
                URL_W_NAME => extracted.url_w = Some(text),
                URL_A_NAME => extracted.url_a = Some(text),
                MOZ_URL_NAME => extracted.moz_url = Some(text),
                _ => extracted.read_errors.push(format!("{name}={text}")),
            }
        }
        Ok(None) => {}
        Err(error) => extracted.read_errors.push(format!("{name}: {error}")),
    }
}

fn read_descriptors(data: &IDataObject, formats: &[ListedFormat], extracted: &mut Extracted) {
    for format in formats {
        let wide = format.name.eq_ignore_ascii_case(FILE_GROUP_W);
        let ansi = format.name.eq_ignore_ascii_case(FILE_GROUP_A);
        if !wide && !ansi {
            continue;
        }
        match read_allowed(data, format.id, &format.name) {
            Ok(Some((bytes, _))) => match if wide {
                file_names_w(&bytes)
            } else {
                file_names_a(&bytes)
            } {
                Ok(names) => {
                    if wide {
                        extracted.virtual_titles.extend(
                            names
                                .iter()
                                .filter_map(|name| title_from_url_filename(name)),
                        );
                    }
                    extracted.virtual_names.extend(names);
                }
                Err(error) => extracted.descriptor_errors.push(error),
            },
            Ok(None) => {}
            Err(error) => extracted.descriptor_errors.push(error),
        }
    }
}

fn uri_list_ids(data: &IDataObject, formats: &[ListedFormat]) -> Vec<(u16, String)> {
    let mut ids = Vec::new();
    for format in formats {
        if format.name.to_ascii_lowercase().contains("uri-list") {
            ids.push((format.id, format.name.clone()));
        }
    }
    if let Some(id) = registered(URI_LIST_NAME)
        && !ids.iter().any(|(existing, _)| *existing == id)
        && query_available(data, id)
    {
        ids.push((id, URI_LIST_NAME.to_string()));
    }
    ids
}

fn find_name(formats: &[ListedFormat], name: &str) -> Option<u16> {
    formats
        .iter()
        .find(|format| format.name.eq_ignore_ascii_case(name))
        .map(|format| format.id)
}

fn probe_known(data: &IDataObject, formats: &mut Vec<ListedFormat>) {
    let mut names = vec![
        (registered(HTML_FORMAT_NAME), HTML_FORMAT_NAME.to_string()),
        (registered(URI_LIST_NAME), URI_LIST_NAME.to_string()),
        (
            registered(FILE_CONTENTS_NAME),
            FILE_CONTENTS_NAME.to_string(),
        ),
        (registered(FILE_GROUP_W), FILE_GROUP_W.to_string()),
        (registered(FILE_GROUP_A), FILE_GROUP_A.to_string()),
        (registered(URL_W_NAME), URL_W_NAME.to_string()),
        (registered(URL_A_NAME), URL_A_NAME.to_string()),
        (registered(MOZ_URL_NAME), MOZ_URL_NAME.to_string()),
        (Some(CF_UNICODETEXT.0), "CF_UNICODETEXT".to_string()),
        (Some(CF_TEXT.0), "CF_TEXT".to_string()),
        (Some(CF_HDROP.0), "CF_HDROP".to_string()),
    ];
    for (id, name) in names.drain(..) {
        let Some(id) = id else {
            continue;
        };
        if id == 0 || formats.iter().any(|format| format.id == id) {
            continue;
        }
        if query_available(data, id) {
            formats.push(ListedFormat { id, name });
        }
    }
}

fn registered(name: &str) -> Option<u16> {
    let wide = utf16(name);
    let id = unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) };
    u16::try_from(id).ok().filter(|id| *id != 0)
}

fn query_available(data: &IDataObject, id: u16) -> bool {
    if id == 0 {
        return false;
    }
    let format = request(id);
    let hr = unsafe { data.QueryGetData(&format) };
    hr == S_OK
}

fn forbidden(id: u16, name: &str) -> bool {
    id == CF_HDROP_ID
        || id == CF_HDROP.0
        || name.eq_ignore_ascii_case(FILE_CONTENTS_NAME)
        || name.eq_ignore_ascii_case("CF_HDROP")
}

fn read_allowed(
    data: &IDataObject,
    id: u16,
    name: &str,
) -> Result<Option<(Vec<u8>, bool)>, String> {
    if forbidden(id, name) {
        return Err(format!("refusing to read {name}"));
    }
    let format = request(id);
    let available = unsafe { data.QueryGetData(&format) };
    if available != S_OK {
        return Ok(None);
    }
    let medium =
        unsafe { data.GetData(&format) }.map_err(|error| format!("GetData {name}: {error}"))?;
    take_bytes(medium).map(Some)
}

fn request(id: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: id,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32 | TYMED_ISTREAM.0 as u32,
    }
}

fn take_bytes(mut medium: STGMEDIUM) -> Result<(Vec<u8>, bool), String> {
    let result = if medium.tymed & (TYMED_HGLOBAL.0 as u32) != 0 {
        let global = unsafe { medium.u.hGlobal };
        hglobal_bytes(global)
    } else if medium.tymed & (TYMED_ISTREAM.0 as u32) != 0 {
        let stream = unsafe { medium.u.pstm.as_ref() };
        match stream {
            Some(stream) => read_stream(stream),
            None => Err("empty IStream".to_string()),
        }
    } else {
        Err(format!("unsupported tymed {}", medium.tymed))
    };
    unsafe { ReleaseStgMedium(&mut medium) };
    result
}

fn hglobal_bytes(global: HGLOBAL) -> Result<(Vec<u8>, bool), String> {
    if global.is_invalid() {
        return Err("null HGLOBAL".to_string());
    }
    let size = unsafe { GlobalSize(global) };
    if size == 0 {
        return Err("empty HGLOBAL".to_string());
    }
    let ptr = unsafe { GlobalLock(global) };
    if ptr.is_null() {
        return Err("GlobalLock failed".to_string());
    }
    let take = size.min(MAX_BYTES);
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), take) }.to_vec();
    let _ = unsafe { GlobalUnlock(global) };
    Ok((bytes, size > MAX_BYTES))
}

fn read_stream(stream: &IStream) -> Result<(Vec<u8>, bool), String> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        if out.len() >= MAX_BYTES {
            return Ok((out, true));
        }
        let space = (MAX_BYTES - out.len()).min(buf.len()) as u32;
        let mut read = 0u32;
        let hr = unsafe { stream.Read(buf.as_mut_ptr().cast(), space, Some(&mut read)) };
        if hr.is_err() {
            return Err(format!("IStream::Read {hr}"));
        }
        if read == 0 {
            break;
        }
        out.extend_from_slice(&buf[..read as usize]);
    }
    Ok((out, false))
}

fn decode_acp(bytes: &[u8]) -> String {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    let bytes = &bytes[..end];
    if bytes.is_empty() {
        return String::new();
    }
    let flags = MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0);
    let needed = unsafe { MultiByteToWideChar(0, flags, bytes, None) };
    if needed <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut wide = vec![0u16; needed as usize];
    let written = unsafe { MultiByteToWideChar(0, flags, bytes, Some(&mut wide)) };
    if written <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    String::from_utf16_lossy(&wide[..written as usize])
}

fn enum_formats(data: &IDataObject) -> Result<Vec<ListedFormat>, String> {
    let enumerator =
        unsafe { data.EnumFormatEtc(1) }.map_err(|error| format!("EnumFormatEtc: {error}"))?;
    let mut formats = Vec::new();
    loop {
        let mut slot = [FORMATETC::default()];
        let mut fetched = 0u32;
        let hr = unsafe { enumerator.Next(&mut slot, Some(&mut fetched)) };
        if hr != S_OK || fetched == 0 {
            break;
        }
        let id = slot[0].cfFormat;
        formats.push(ListedFormat {
            id,
            name: format_name(id),
        });
    }
    Ok(formats)
}

fn format_name(id: u16) -> String {
    if id == CF_TEXT.0 {
        return "CF_TEXT".to_string();
    }
    if id == CF_UNICODETEXT.0 {
        return "CF_UNICODETEXT".to_string();
    }
    if id == CF_HDROP.0 {
        return "CF_HDROP".to_string();
    }
    let mut buf = [0u16; 260];
    let written = unsafe { GetClipboardFormatNameW(u32::from(id), &mut buf) };
    if written <= 0 {
        return format!("format-id-{id}");
    }
    let len = (written as usize).min(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

#[implement(IDataObject)]
struct MemData {
    items: Vec<(u16, Vec<u8>)>,
    gets: Arc<Mutex<Vec<u16>>>,
}

#[implement(IEnumFORMATETC)]
struct FormatEnum {
    items: Vec<FORMATETC>,
    cursor: AtomicUsize,
}

fn medium_from_bytes(bytes: &[u8]) -> windows::core::Result<STGMEDIUM> {
    let global = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) }?;
    let ptr = unsafe { GlobalLock(global) };
    if ptr.is_null() {
        return Err(E_NOTIMPL.into());
    }
    unsafe {
        if !bytes.is_empty() {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr.cast(), bytes.len());
        }
        let _ = GlobalUnlock(global);
    }
    let mut medium = STGMEDIUM {
        tymed: TYMED_HGLOBAL.0 as u32,
        ..Default::default()
    };
    medium.u.hGlobal = global;
    Ok(medium)
}

#[allow(non_snake_case)]
impl IDataObject_Impl for MemData_Impl {
    fn GetData(&self, format: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        let hr = self.QueryGetData(format);
        if hr != S_OK {
            return Err(hr.into());
        }
        let format = unsafe { &*format };
        self.gets
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .push(format.cfFormat);
        let bytes = self
            .items
            .iter()
            .find(|(id, _)| *id == format.cfFormat)
            .map(|(_, bytes)| bytes.clone())
            .unwrap_or_default();
        medium_from_bytes(&bytes)
    }

    fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn QueryGetData(&self, format: *const FORMATETC) -> HRESULT {
        if format.is_null() {
            return E_INVALIDARG;
        }
        let format = unsafe { &*format };
        let known = self.items.iter().any(|(id, _)| *id == format.cfFormat);
        if !known {
            return DV_E_FORMATETC;
        }
        let tymed = format.tymed;
        if tymed != 0 && tymed & (TYMED_HGLOBAL.0 as u32) == 0 {
            return DV_E_TYMED;
        }
        S_OK
    }

    fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, _: *mut FORMATETC) -> HRESULT {
        DV_E_FORMATETC
    }

    fn SetData(
        &self,
        _: *const FORMATETC,
        _: *const STGMEDIUM,
        _: BOOL,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn EnumFormatEtc(&self, direction: u32) -> windows::core::Result<IEnumFORMATETC> {
        if direction != 1 {
            return Err(E_NOTIMPL.into());
        }
        let items = self
            .items
            .iter()
            .map(|(id, _)| FORMATETC {
                cfFormat: *id,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as u32,
            })
            .collect();
        Ok(FormatEnum {
            items,
            cursor: AtomicUsize::new(0),
        }
        .into())
    }

    fn DAdvise(
        &self,
        _: *const FORMATETC,
        _: u32,
        _: Ref<IAdviseSink>,
    ) -> windows::core::Result<u32> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn DUnadvise(&self, _: u32) -> windows::core::Result<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}

#[allow(non_snake_case)]
impl IEnumFORMATETC_Impl for FormatEnum_Impl {
    fn Next(&self, celt: u32, rgelt: *mut FORMATETC, fetched: *mut u32) -> HRESULT {
        if celt == 0 {
            if !fetched.is_null() {
                unsafe { *fetched = 0 };
            }
            return S_OK;
        }
        if rgelt.is_null() {
            return E_INVALIDARG;
        }
        let cursor = self.cursor.load(Ordering::Relaxed);
        let available = self.items.len().saturating_sub(cursor);
        let take = usize::try_from(celt).unwrap_or(0).min(available);
        unsafe {
            for (index, item) in self.items[cursor..cursor + take].iter().enumerate() {
                rgelt.add(index).write(*item);
            }
            if !fetched.is_null() {
                *fetched = u32::try_from(take).unwrap_or(u32::MAX);
            }
        }
        self.cursor.store(cursor + take, Ordering::Relaxed);
        if take == celt as usize { S_OK } else { S_FALSE }
    }

    fn Skip(&self, celt: u32) -> windows::core::Result<()> {
        let cursor = self.cursor.load(Ordering::Relaxed);
        let next = cursor.saturating_add(celt as usize);
        if next > self.items.len() {
            self.cursor.store(self.items.len(), Ordering::Relaxed);
            return Err(S_FALSE.into());
        }
        self.cursor.store(next, Ordering::Relaxed);
        Ok(())
    }

    fn Reset(&self) -> windows::core::Result<()> {
        self.cursor.store(0, Ordering::Relaxed);
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IEnumFORMATETC> {
        Ok(FormatEnum {
            items: self.items.clone(),
            cursor: AtomicUsize::new(self.cursor.load(Ordering::Relaxed)),
        }
        .into())
    }
}

pub(crate) fn check_synthetic_drop() -> Result<(), String> {
    ensure_ole()?;
    let html_id = must_format(HTML_FORMAT_NAME)?;
    let uri_id = must_format(URI_LIST_NAME)?;
    let group_id = must_format(FILE_GROUP_W)?;
    let contents_id = must_format(FILE_CONTENTS_NAME)?;
    let url_w_id = must_format(URL_W_NAME)?;
    let href = "https://example.com/a/?q=1";
    let gets = Arc::new(Mutex::new(Vec::new()));
    let items = vec![
        (html_id, parse::chrome_like_cf_html(href, href, "示例标题")),
        (
            uri_id,
            b"# comment\r\nhttps://example.com/a/?q=1\r\nhttps://example.com/b\r\n".to_vec(),
        ),
        (CF_UNICODETEXT.0, parse::encode_utf16_text(href)),
        (CF_TEXT.0, b"https://example.com/a/?q=1\0".to_vec()),
        (url_w_id, parse::encode_utf16_text(href)),
        (group_id, parse::descriptor_w(&["\u{9875}\u{9762}.url"])),
        (
            contents_id,
            b"[InternetShortcut]\r\nURL=https://secret.example/hidden\r\n".to_vec(),
        ),
        (CF_HDROP.0, b"C:\\should-not-read.url\0".to_vec()),
    ];
    let data: IDataObject = MemData {
        items,
        gets: gets.clone(),
    }
    .into();
    let dir = std::env::temp_dir().join(format!(
        "lanwork-urldrop-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let log_path = dir.join("drop.log");
    let state = Arc::new(DropState {
        log: Mutex::new(String::new()),
        edit: AtomicIsize::new(0),
        log_path: log_path.clone(),
    });
    let target = drop_target(state.clone());
    let point = windows::Win32::Foundation::POINTL { x: 0, y: 0 };
    let keys = windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS(0);
    let mut effect = DROPEFFECT(EFFECT_COPY | EFFECT_MOVE | EFFECT_LINK);
    unsafe { target.DragEnter(&data, keys, point, &mut effect) }
        .map_err(|error| error.to_string())?;
    if effect.0 != EFFECT_COPY {
        return Err(format!("DragEnter effect {}", effect.0));
    }
    effect = DROPEFFECT(EFFECT_MOVE);
    unsafe { target.DragOver(keys, point, &mut effect) }.map_err(|error| error.to_string())?;
    if effect.0 != 0 {
        return Err(format!("DragOver accepted move {}", effect.0));
    }
    effect = DROPEFFECT(EFFECT_COPY | EFFECT_MOVE);
    unsafe { target.Drop(&data, keys, point, &mut effect) }.map_err(|error| error.to_string())?;
    if effect.0 != EFFECT_COPY {
        return Err(format!("Drop effect {}", effect.0));
    }
    effect = DROPEFFECT(EFFECT_MOVE);
    unsafe { target.Drop(&data, keys, point, &mut effect) }.map_err(|error| error.to_string())?;
    if effect.0 != 0 {
        return Err("move-only drop returned an effect".to_string());
    }
    let recorded = gets
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    if recorded
        .iter()
        .any(|id| *id == contents_id || *id == CF_HDROP.0)
    {
        return Err(format!("virtual file was read: {recorded:?}"));
    }
    let log = std::fs::read_to_string(&log_path).map_err(|error| error.to_string())?;
    for phrase in [
        "kind=drop",
        "chosen-effect=copy",
        "chosen-effect=none",
        "cf-html-link-1-href=https://example.com/a/?q=1",
        "cf-html-link-1-text=示例标题",
        "text-uri-list-present=true",
        "text-uri-list-1-url-1=https://example.com/a/?q=1",
        "cf-unicodetext=https://example.com/a/?q=1",
        "cf-text=https://example.com/a/?q=1",
        "UniformResourceLocatorW=https://example.com/a/?q=1",
        "virtual-url-name-1=页面.url",
        "descriptor-title-1=页面",
        "file-contents-offered=true",
        "file-contents-read=false",
        "file-contents-written=false",
        "cf-hdrop-offered=true",
        "hdrop-read=false",
        "virtual-url-stored=false",
        "virtual-url-copied=false",
        "browser-control-called=false",
        "format-priority=not-decided",
    ] {
        if !log.contains(phrase) {
            return Err(format!("synthetic log missing {phrase}\n{log}"));
        }
    }
    if log.contains("secret.example") || log.contains("should-not-read") {
        return Err("virtual .url bytes were copied into the log".to_string());
    }
    let mut extras = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        extras.push(entry.file_name());
    }
    if extras.len() != 1 {
        return Err(format!("drop wrote unexpected files: {extras:?}"));
    }
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

fn must_format(name: &str) -> Result<u16, String> {
    registered(name).ok_or_else(|| format!("RegisterClipboardFormat {name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_drop_reads_urls_and_does_not_copy_virtual_url() {
        check_synthetic_drop().expect("synthetic drop");
    }
}
