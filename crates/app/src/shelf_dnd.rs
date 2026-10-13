//! 收纳页的原生 OLE 拖入和拖出。判定规则在 `lanwork_core::panel`（`drop_effect`、`continue_drag` 等），
//! 这里只接 Win32。文件拖放来自 `spikes/dnd`，网址读取来自 `spikes/urldrop`。
//!
//! 放下目标注册在面板和收纳悬浮窗的窗口句柄上。只在接受区域内、数据里有文件或可识别的网址时才接受。
//! 允许的效果只有复制和快捷方式，不返回移动。路径只读，不打开、不复制、不移动、不删除用户文件。
//! `DragEnter` 只 `QueryGetData`。`Drop` 才 `GetData`，先试网址格式，再读路径。
//! 描述符里有以 `.url` 结尾的文件名、但解析不出网址时拒绝，不把虚拟 `.url` 存成路径。不读 `FileContents`。

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lanwork_core::panel::{
    DragContinue, DropZone, EFFECT_NONE, OUTGOING_EFFECTS, continue_drag, drop_effect,
};
use lanwork_core::shelves::{
    PickedWebRef, WebDropInput, decode_utf16_text, file_names_w, pick_web_drop,
    title_from_url_filename,
};
use windows::Win32::Foundation::{
    DRAGDROP_E_ALREADYREGISTERED, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS,
    HGLOBAL, HWND, POINT, POINTL, S_OK,
};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{
    CoTaskMemFree, DVASPECT_CONTENT, FORMATETC, IDataObject, IStream, STGMEDIUM, TYMED_HGLOBAL,
    TYMED_ISTREAM,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{
    CF_HDROP, CF_UNICODETEXT, DROPEFFECT, DoDragDrop, IDropSource, IDropSource_Impl, IDropTarget,
    IDropTarget_Impl, OleInitialize, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, DragQueryFileW, HDROP, ILFree, IShellItemArray,
    SHCreateShellItemArrayFromDataObject, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
    SIGDN_FILESYSPATH,
};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXDRAG, SM_CYDRAG};
use windows::core::{BOOL, HRESULT, PCWSTR, implement};

/// 一次放下。网址优先：已经读出网址时不再读 `CF_HDROP` 或 `FileContents`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IncomingDrop {
    Paths(Vec<PathBuf>),
    Web { url: String, title: String },
}

/// 放下目标读到路径或网址之后调用。在界面线程上、OLE 的放下回调里执行，所以只应登记，不应做耗时的事。
pub(crate) type DropSink = Arc<dyn Fn(IncomingDrop) + Send + Sync>;

/// 放下目标要读的状态。界面线程更新，放下回调读取。
#[derive(Default)]
pub(crate) struct DropState {
    hwnd: AtomicIsize,
    accepting: AtomicBool,
    zone: Mutex<DropZone>,
}

impl DropState {
    pub(crate) fn set_accepting(&self, accepting: bool) {
        self.accepting.store(accepting, Ordering::Release);
    }

    pub(crate) fn set_zone(&self, zone: DropZone) {
        *self.zone.lock().unwrap_or_else(PoisonError::into_inner) = zone;
    }

    fn set_hwnd(&self, hwnd: HWND) {
        self.hwnd.store(hwnd.0 as isize, Ordering::Release);
    }

    fn hwnd(&self) -> HWND {
        HWND(self.hwnd.load(Ordering::Acquire) as *mut std::ffi::c_void)
    }

    fn effect(&self, point: &POINTL, has_files: bool, offered: u32) -> u32 {
        let (x, y) = client_point(self.hwnd(), point);
        let zone = *self.zone.lock().unwrap_or_else(PoisonError::into_inner);
        drop_effect(
            self.accepting.load(Ordering::Acquire),
            zone.contains(x, y),
            has_files,
            offered,
        )
    }
}

/// 屏幕坐标换成客户区物理像素。没有窗口句柄（测试）时原样返回。
fn client_point(hwnd: HWND, point: &POINTL) -> (i32, i32) {
    if hwnd.0.is_null() {
        return (point.x, point.y);
    }
    let mut client = POINT {
        x: point.x,
        y: point.y,
    };
    // SAFETY: hwnd 是本进程的窗口，输出指针指向局部变量。失败时 client 保持屏幕坐标。
    let _ = unsafe { ScreenToClient(hwnd, &mut client) };
    (client.x, client.y)
}

fn read_effect(slot: *mut DROPEFFECT) -> u32 {
    if slot.is_null() {
        return EFFECT_NONE;
    }
    // SAFETY: OLE 在这次调用期间给出有效、可读的效果指针，上面已排除空指针。
    unsafe { (*slot).0 }
}

fn write_effect(slot: *mut DROPEFFECT, effect: u32) {
    if slot.is_null() {
        return;
    }
    // SAFETY: OLE 在这次调用期间给出有效、可写的效果指针，上面已排除空指针。
    unsafe { *slot = DROPEFFECT(effect) }
}

#[implement(IDropTarget)]
struct ShelfDrop {
    state: Arc<DropState>,
    sink: DropSink,
    /// 这一轮 `DragEnter` 看到数据里有文件。`DragOver` 没有数据对象，靠它判断。
    has_files: AtomicBool,
    /// 这一轮 `DragEnter` 用 `QueryGetData` 看到网址格式或 `FileGroupDescriptorW`。不调用 `GetData`。
    has_web: AtomicBool,
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for ShelfDrop_Impl {
    fn DragEnter(
        &self,
        data: windows::core::Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let files = data.as_ref().is_some_and(has_hdrop);
        let web = data.as_ref().is_some_and(offers_web);
        self.has_files.store(files, Ordering::Release);
        self.has_web.store(web, Ordering::Release);
        write_effect(
            effect,
            self.state.effect(point, files || web, read_effect(effect)),
        );
        Ok(())
    }

    fn DragOver(
        &self,
        _keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let payload =
            self.has_files.load(Ordering::Acquire) || self.has_web.load(Ordering::Acquire);
        write_effect(
            effect,
            self.state.effect(point, payload, read_effect(effect)),
        );
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        self.has_files.store(false, Ordering::Release);
        self.has_web.store(false, Ordering::Release);
        Ok(())
    }

    fn Drop(
        &self,
        data: windows::core::Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let cached_web = self.has_web.swap(false, Ordering::AcqRel);
        self.has_files.store(false, Ordering::Release);
        let files = data.as_ref().is_some_and(has_hdrop);
        let chosen = self
            .state
            .effect(point, files || cached_web, read_effect(effect));
        write_effect(effect, chosen);
        if chosen == EFFECT_NONE {
            return Ok(());
        }
        let Some(data) = data.as_ref() else {
            write_effect(effect, EFFECT_NONE);
            return Ok(());
        };
        let read = read_web_drop(data);
        if let Some(picked) = read.picked {
            (self.sink)(IncomingDrop::Web {
                url: picked.url,
                title: picked.title,
            });
            return Ok(());
        }
        // 浏览器拖放的虚拟 .url 写在 FileGroupDescriptorW 里。解析不出网址就拒绝，不把这份虚拟文件存成路径。
        // 资源管理器拖普通文件时也会带这个格式，文件名不是 .url，仍按路径引用处理。
        if read.virtual_url {
            write_effect(effect, EFFECT_NONE);
            return Ok(());
        }
        match paths_from_data_object(data) {
            Ok(paths) if !paths.is_empty() => (self.sink)(IncomingDrop::Paths(paths)),
            _ => write_effect(effect, EFFECT_NONE),
        }
        Ok(())
    }
}

fn hdrop_format() -> FORMATETC {
    FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn has_hdrop(data: &IDataObject) -> bool {
    let format = hdrop_format();
    // SAFETY: format 是局部变量，调用期间有效；只查询格式，不取数据。
    unsafe { data.QueryGetData(&format) }.is_ok()
}

fn query_clipboard(data: &IDataObject, id: u16) -> bool {
    if id == 0 {
        return false;
    }
    let format = FORMATETC {
        cfFormat: id,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32 | TYMED_ISTREAM.0 as u32,
    };
    // SAFETY: format 指向局部变量。只查询，不取数据。
    unsafe { data.QueryGetData(&format) }.is_ok()
}

/// `DragEnter` 用来决定光标。只 `QueryGetData`，不 `GetData`。
fn offers_web(data: &IDataObject) -> bool {
    let ids = web_format_ids();
    query_clipboard(data, ids.html)
        || query_clipboard(data, ids.url_w)
        || query_clipboard(data, ids.moz)
        || query_clipboard(data, CF_UNICODETEXT.0)
        || query_clipboard(data, ids.descriptor)
}

const WEB_READ_LIMIT: usize = 1024 * 1024;

struct WebFormatIds {
    html: u16,
    url_w: u16,
    moz: u16,
    descriptor: u16,
}

fn web_format_ids() -> &'static WebFormatIds {
    static IDS: OnceLock<WebFormatIds> = OnceLock::new();
    IDS.get_or_init(|| WebFormatIds {
        html: register_clipboard_format("HTML Format"),
        url_w: register_clipboard_format("UniformResourceLocatorW"),
        moz: register_clipboard_format("text/x-moz-url"),
        descriptor: register_clipboard_format("FileGroupDescriptorW"),
    })
}

fn register_clipboard_format(name: &str) -> u16 {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: wide 以 0 结尾，调用期间有效。失败时返回 0。
    let id = unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) };
    u16::try_from(id).unwrap_or(0)
}

struct WebRead {
    picked: Option<PickedWebRef>,
    /// 描述符里有以 `.url` 结尾的文件名。这是浏览器拖放的虚拟文件，不是资源管理器里的普通文件。
    virtual_url: bool,
}

/// 只读已经接受的网址格式和 `FileGroupDescriptorW`。不查询、不读取 `FileContents` 和 `CF_HDROP`。
fn read_web_drop(data: &IDataObject) -> WebRead {
    let ids = web_format_ids();
    let html = read_clipboard_bytes(data, ids.html);
    let url_w = read_clipboard_bytes(data, ids.url_w).map(|bytes| decode_utf16_text(&bytes));
    let moz = read_clipboard_bytes(data, ids.moz).map(|bytes| decode_utf16_text(&bytes));
    let unicode =
        read_clipboard_bytes(data, CF_UNICODETEXT.0).map(|bytes| decode_utf16_text(&bytes));
    let names = read_clipboard_bytes(data, ids.descriptor)
        .and_then(|bytes| file_names_w(&bytes).ok())
        .unwrap_or_default();
    let virtual_url = names
        .iter()
        .any(|name| title_from_url_filename(name).is_some());
    WebRead {
        picked: pick_web_drop(&WebDropInput {
            html: html.as_deref(),
            url_w: url_w.as_deref(),
            moz_url: moz.as_deref(),
            unicode_text: unicode.as_deref(),
            descriptor_names: &names,
        }),
        virtual_url,
    }
}

fn read_clipboard_bytes(data: &IDataObject, id: u16) -> Option<Vec<u8>> {
    if id == 0 || id == CF_HDROP.0 {
        return None;
    }
    let format = FORMATETC {
        cfFormat: id,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32 | TYMED_ISTREAM.0 as u32,
    };
    // SAFETY: format 指向局部变量。失败表示没有这种格式。
    if unsafe { data.QueryGetData(&format) }.is_err() {
        return None;
    }
    // SAFETY: 同上。GetData 成功时 medium 由调用方释放。
    let mut medium = unsafe { data.GetData(&format) }.ok()?;
    let bytes = medium_bytes(&medium);
    // SAFETY: medium 来自本次 GetData，还没有释放。
    unsafe { ReleaseStgMedium(&mut medium) };
    bytes
}

fn medium_bytes(medium: &STGMEDIUM) -> Option<Vec<u8>> {
    if medium.tymed & (TYMED_HGLOBAL.0 as u32) != 0 {
        // SAFETY: tymed 标明这是 HGLOBAL。联合体字段在这次调用期间有效。
        let global = unsafe { medium.u.hGlobal };
        return hglobal_bytes(global);
    }
    if medium.tymed & (TYMED_ISTREAM.0 as u32) != 0 {
        // SAFETY: tymed 标明这是 IStream。
        let stream = unsafe { medium.u.pstm.as_ref() }?;
        return read_stream(stream).ok();
    }
    None
}

fn hglobal_bytes(global: HGLOBAL) -> Option<Vec<u8>> {
    if global.is_invalid() {
        return None;
    }
    // SAFETY: global 来自 STGMEDIUM，调用期间有效。
    let size = unsafe { GlobalSize(global) };
    if size == 0 {
        return None;
    }
    // SAFETY: 同上。失败时指针为空。
    let ptr = unsafe { GlobalLock(global) };
    if ptr.is_null() {
        return None;
    }
    let take = size.min(WEB_READ_LIMIT);
    // SAFETY: GlobalLock 成功，可读 take 个字节，不超过分配大小。
    let bytes = unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), take) }.to_vec();
    // SAFETY: 与上面的 GlobalLock 成对。
    let _ = unsafe { GlobalUnlock(global) };
    Some(bytes)
}

fn read_stream(stream: &IStream) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        if out.len() >= WEB_READ_LIMIT {
            return Ok(out);
        }
        let space = u32::try_from((WEB_READ_LIMIT - out.len()).min(buf.len())).unwrap_or(0);
        let mut read = 0u32;
        // SAFETY: buf 可写 space 个字节，read 指向局部变量。
        let hr = unsafe { stream.Read(buf.as_mut_ptr().cast(), space, Some(&mut read)) };
        if hr.is_err() {
            return Err(format!("IStream::Read {hr}"));
        }
        if read == 0 {
            break;
        }
        let count = usize::try_from(read).unwrap_or(0).min(buf.len());
        out.extend_from_slice(&buf[..count]);
    }
    Ok(out)
}

/// 先用外壳项取路径：外壳用固定 260 单元组 `CF_HDROP`，路径更长时 `GetData` 要么失败（0x8007007A），
/// 要么给出 8.3 短名。外壳项给出完整的文件系统路径。取不到时再读 `CF_HDROP`。
fn paths_from_data_object(data: &IDataObject) -> Result<Vec<PathBuf>, String> {
    let shell_error = match paths_from_shell_items(data) {
        Ok(paths) if !paths.is_empty() => return Ok(paths),
        Ok(_) => "没有条目".to_owned(),
        Err(error) => error,
    };
    let format = hdrop_format();
    // SAFETY: format 是局部变量，调用期间有效。
    match unsafe { data.GetData(&format) } {
        Ok(mut medium) => {
            // SAFETY: CF_HDROP 的存储介质是 HGLOBAL，所以联合体里读 hGlobal 是有效的。
            let hdrop = HDROP(unsafe { medium.u.hGlobal }.0);
            let paths = paths_from_hdrop(hdrop);
            // SAFETY: medium 由上面的 GetData 填写，此后不再使用。
            unsafe { ReleaseStgMedium(&mut medium) };
            Ok(paths)
        }
        Err(error) => Err(format!(
            "读取 CF_HDROP 失败：{error}；外壳项：{shell_error}"
        )),
    }
}

fn paths_from_hdrop(hdrop: HDROP) -> Vec<PathBuf> {
    // SAFETY: hdrop 来自仍然有效的存储介质；索引 u32::MAX 只问个数。
    let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
    let mut paths = Vec::with_capacity(count as usize);
    for index in 0..count {
        // SAFETY: 不给缓冲区时只返回该项需要的长度（不含结尾的零）。
        let len = unsafe { DragQueryFileW(hdrop, index, None) };
        let mut buffer = vec![0u16; len as usize + 1];
        // SAFETY: 缓冲区比该项的长度多一个单元，足够放下路径和结尾的零。
        let copied = unsafe { DragQueryFileW(hdrop, index, Some(&mut buffer)) };
        let end = (copied as usize).min(buffer.len());
        let end = buffer[..end]
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(end);
        paths.push(PathBuf::from(OsString::from_wide(&buffer[..end])));
    }
    paths
}

fn paths_from_shell_items(data: &IDataObject) -> Result<Vec<PathBuf>, String> {
    // SAFETY: data 是调用方持有的有效数据对象。
    let items: IShellItemArray = unsafe { SHCreateShellItemArrayFromDataObject(data) }
        .map_err(|error| format!("SHCreateShellItemArrayFromDataObject：{error}"))?;
    // SAFETY: items 是上面刚得到的有效接口。
    let count = unsafe { items.GetCount() }.map_err(|error| format!("GetCount：{error}"))?;
    let mut paths = Vec::with_capacity(count as usize);
    for index in 0..count {
        // SAFETY: index 小于 GetCount 返回的个数。
        let item =
            unsafe { items.GetItemAt(index) }.map_err(|error| format!("GetItemAt：{error}"))?;
        // SAFETY: item 是有效接口；返回的字符串由下面的 CoTaskMemFree 释放。
        let name = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }
            .map_err(|error| format!("GetDisplayName：{error}"))?;
        let path = path_from_pwstr(name);
        // SAFETY: name 由 GetDisplayName 用 CoTaskMemAlloc 分配，这里只释放一次，之后不再读。
        unsafe { CoTaskMemFree(Some(name.0.cast())) };
        if path.as_os_str().is_empty() {
            return Err(format!("第 {index} 项没有文件系统路径"));
        }
        paths.push(path);
    }
    Ok(paths)
}

fn path_from_pwstr(name: windows::core::PWSTR) -> PathBuf {
    if name.0.is_null() {
        return PathBuf::new();
    }
    // SAFETY: 外壳返回以零结尾的宽字符串；只读到结尾的零为止。
    let wide = unsafe { name.as_wide() };
    let verbatim: [u16; 4] = [
        u16::from(b'\\'),
        u16::from(b'\\'),
        u16::from(b'?'),
        u16::from(b'\\'),
    ];
    let wide = wide.strip_prefix(&verbatim).unwrap_or(wide);
    PathBuf::from(OsString::from_wide(wide))
}

/// 放下目标登记。丢掉时撤销登记。必须在登记它的线程上丢掉。
pub(crate) struct Registration {
    hwnd: HWND,
    _target: IDropTarget,
}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: hwnd 是登记时的窗口；窗口已销毁时调用只返回错误，结果不使用。
        let _ = unsafe { RevokeDragDrop(self.hwnd) };
    }
}

/// 在面板窗口上登记放下目标。winit 创建窗口时已经登记了自己的目标，先撤销再登记。
pub(crate) fn register(
    hwnd: HWND,
    state: Arc<DropState>,
    sink: DropSink,
) -> Result<Registration, String> {
    // SAFETY: winit 创建窗口时已经初始化 OLE；重复调用只返回 S_FALSE。
    let _ = unsafe { OleInitialize(None) };
    state.set_hwnd(hwnd);
    let target: IDropTarget = ShelfDrop {
        state,
        sink,
        has_files: AtomicBool::new(false),
        has_web: AtomicBool::new(false),
    }
    .into();
    // SAFETY: hwnd 是本进程、本线程创建的窗口，target 是有效接口，登记会自己持有一份引用。
    match unsafe { RegisterDragDrop(hwnd, &target) } {
        Ok(()) => {}
        Err(error) if error.code() == DRAGDROP_E_ALREADYREGISTERED => {
            // SAFETY: 同上；撤销的是 winit 登记的目标，之后立即换成我们的。
            unsafe { RevokeDragDrop(hwnd) }.map_err(|error| format!("RevokeDragDrop：{error}"))?;
            // SAFETY: 同上。
            unsafe { RegisterDragDrop(hwnd, &target) }
                .map_err(|error| format!("RegisterDragDrop：{error}"))?;
        }
        Err(error) => return Err(format!("RegisterDragDrop：{error}")),
    }
    Ok(Registration {
        hwnd,
        _target: target,
    })
}

/// 系统的拖动阈值，物理像素。
pub(crate) fn drag_metrics() -> (i32, i32) {
    // SAFETY: 只读系统度量，没有指针参数。
    unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) }
}

#[implement(IDropSource)]
struct ShelfSource;

#[allow(non_snake_case)]
impl IDropSource_Impl for ShelfSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        let left_down = keys.0 & MK_LBUTTON.0 != 0;
        match continue_drag(escape.as_bool(), left_down) {
            DragContinue::Continue => S_OK,
            DragContinue::Drop => DRAGDROP_S_DROP,
            DragContinue::Cancel => DRAGDROP_S_CANCEL,
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

struct ParsedId(*mut ITEMIDLIST);

impl Drop for ParsedId {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: 指针由 SHParseDisplayName 分配，这里只释放一次。
            unsafe { ILFree(Some(self.0.cast())) };
        }
    }
}

/// 一组路径的外壳数据对象。路径原样交给外壳，不解析快捷方式。
fn data_object_for_paths(paths: &[PathBuf]) -> Result<IDataObject, String> {
    let mut parsed = Vec::with_capacity(paths.len());
    for path in paths {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut pidl = std::ptr::null_mut();
        // SAFETY: wide 以零结尾并在调用期间有效；pidl 指向局部变量，成功后交给 ParsedId 释放。
        unsafe {
            SHParseDisplayName(
                PCWSTR(wide.as_ptr()),
                None::<&windows::Win32::System::Com::IBindCtx>,
                &mut pidl,
                0,
                None,
            )
        }
        .map_err(|error| format!("SHParseDisplayName {}：{error}", path.display()))?;
        parsed.push(ParsedId(pidl));
    }
    let raw: Vec<*const ITEMIDLIST> = parsed.iter().map(|item| item.0.cast_const()).collect();
    // SAFETY: raw 里的指针都由上面的 ParsedId 持有，在本函数返回前不会释放。
    let items: IShellItemArray = unsafe { SHCreateShellItemArrayFromIDLists(&raw) }
        .map_err(|error| format!("SHCreateShellItemArrayFromIDLists：{error}"))?;
    // SAFETY: items 是上面刚得到的有效接口。
    unsafe {
        items.BindToHandler::<_, IDataObject>(
            None::<&windows::Win32::System::Com::IBindCtx>,
            &BHID_DataObject,
        )
    }
    .map_err(|error| format!("BindToHandler：{error}"))
}

/// 一次拖出的结果。
#[derive(Debug, Clone, Copy)]
pub(crate) struct DragOutcome {
    pub(crate) result: HRESULT,
}

impl DragOutcome {
    /// 用户取消或放下都不是错误；其他 HRESULT 才是。
    pub(crate) fn failed(self) -> bool {
        self.result != DRAGDROP_S_DROP && self.result != DRAGDROP_S_CANCEL && self.result.is_err()
    }
}

/// 开始拖出这些路径。允许的效果只有复制和快捷方式。调用会进入 OLE 的模态循环，直到放下或取消。
pub(crate) fn drag_out(paths: &[PathBuf]) -> Result<DragOutcome, String> {
    let data = data_object_for_paths(paths)?;
    let source: IDropSource = ShelfSource.into();
    let mut effect = DROPEFFECT(EFFECT_NONE);
    // SAFETY: data 和 source 是有效接口，effect 指向局部变量；允许的效果不含移动。
    let result = unsafe { DoDragDrop(&data, &source, DROPEFFECT(OUTGOING_EFFECTS), &mut effect) };
    Ok(DragOutcome { result })
}

/// 路径转成可保存的字符串。不是合法 Unicode 的路径返回 `None`。
pub(crate) fn path_text(path: &Path) -> Option<String> {
    path.to_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use lanwork_core::panel::{
        EFFECT_COPY, EFFECT_LINK, EFFECT_MOVE, drop_effect as pure_drop_effect,
    };
    use windows::Win32::System::Ole::{
        DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
    };

    use super::*;

    fn ensure_ole() {
        // SAFETY: 测试线程自己初始化 OLE；重复调用只返回 S_FALSE。
        unsafe { OleInitialize(None) }.expect("OleInitialize");
    }

    fn scratch(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("lanwork-shelf-dnd-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch dir");
        root
    }

    fn same_file(left: &Path, right: &Path) -> bool {
        matches!(
            (std::fs::canonicalize(left), std::fs::canonicalize(right)),
            (Ok(left), Ok(right)) if left == right
        )
    }

    fn open_zone() -> DropZone {
        DropZone {
            x: 0.0,
            y: 0.0,
            width: 400.0,
            height: 300.0,
            scale: 1.0,
        }
    }

    #[test]
    fn effect_constants_match_win32() {
        assert_eq!(DROPEFFECT_NONE.0, EFFECT_NONE);
        assert_eq!(DROPEFFECT_COPY.0, EFFECT_COPY);
        assert_eq!(DROPEFFECT_MOVE.0, EFFECT_MOVE);
        assert_eq!(DROPEFFECT_LINK.0, EFFECT_LINK);
        assert_eq!(pure_drop_effect(true, true, true, EFFECT_MOVE), EFFECT_NONE);
    }

    #[test]
    fn shell_data_object_round_trips_files_and_folders() {
        ensure_ole();
        let root = scratch("roundtrip");
        let file = root.join("readme.txt");
        std::fs::write(&file, b"readme").expect("file");
        let folder = root.join("folder");
        std::fs::create_dir(&folder).expect("folder");
        let data = data_object_for_paths(&[file.clone(), folder.clone()]).expect("data object");
        assert!(has_hdrop(&data));
        let read = paths_from_data_object(&data).expect("paths");
        assert_eq!(read.len(), 2);
        assert!(same_file(&read[0], &file), "{read:?}");
        assert!(same_file(&read[1], &folder), "{read:?}");
        assert_eq!(std::fs::read(&file).expect("file still there"), b"readme");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn shell_data_object_reads_back_a_path_longer_than_260_units() {
        ensure_ole();
        let root = scratch("long");
        let mut nested = root.clone();
        while nested.as_os_str().encode_wide().count() <= 230 {
            nested.push("d".repeat(40));
        }
        std::fs::create_dir_all(&nested).expect("long dir");
        let file = nested.join("long.txt");
        std::fs::write(&file, b"long").expect("long file");
        assert!(file.as_os_str().encode_wide().count() > 260);
        let data = data_object_for_paths(std::slice::from_ref(&file)).expect("data object");
        let read = paths_from_data_object(&data).expect("paths");
        assert_eq!(read.len(), 1);
        assert!(same_file(&read[0], &file), "{read:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    struct Probe {
        target: IDropTarget,
        state: Arc<DropState>,
        received: mpsc::Receiver<IncomingDrop>,
    }

    fn probe() -> Probe {
        let state = Arc::new(DropState::default());
        state.set_zone(open_zone());
        let (send, received) = mpsc::channel();
        let send = Mutex::new(send);
        let sink: DropSink = Arc::new(move |incoming| {
            let _ = send
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send(incoming);
        });
        let target: IDropTarget = ShelfDrop {
            state: Arc::clone(&state),
            sink,
            has_files: AtomicBool::new(false),
            has_web: AtomicBool::new(false),
        }
        .into();
        Probe {
            target,
            state,
            received,
        }
    }

    fn offer(probe: &Probe, data: &IDataObject, x: i32, offered: u32) -> u32 {
        let point = POINTL { x, y: 10 };
        let mut effect = DROPEFFECT(offered);
        // SAFETY: data 是有效数据对象，point 和 effect 指向局部变量。
        unsafe {
            probe
                .target
                .DragEnter(data, MODIFIERKEYS_FLAGS(0), point, &mut effect)
        }
        .expect("DragEnter");
        effect.0
    }

    #[test]
    fn drop_target_accepts_only_on_the_shelf_tab_inside_the_zone_and_never_moves() {
        ensure_ole();
        let root = scratch("target");
        let file = root.join("a.txt");
        std::fs::write(&file, b"a").expect("file");
        let data = data_object_for_paths(std::slice::from_ref(&file)).expect("data object");
        let all = EFFECT_COPY | EFFECT_MOVE | EFFECT_LINK;
        let probe = probe();

        assert_eq!(offer(&probe, &data, 10, all), EFFECT_NONE, "not accepting");
        probe.state.set_accepting(true);
        assert_eq!(offer(&probe, &data, 10, all), EFFECT_COPY);
        assert_eq!(
            offer(&probe, &data, 10, EFFECT_MOVE | EFFECT_LINK),
            EFFECT_LINK
        );
        assert_eq!(offer(&probe, &data, 10, EFFECT_MOVE), EFFECT_NONE);
        assert_eq!(
            offer(&probe, &data, 500, all),
            EFFECT_NONE,
            "outside the zone"
        );

        let point = POINTL { x: 10, y: 10 };
        let mut effect = DROPEFFECT(all);
        // SAFETY: data 是有效数据对象，point 和 effect 指向局部变量。
        unsafe {
            probe
                .target
                .Drop(&data, MODIFIERKEYS_FLAGS(0), point, &mut effect)
        }
        .expect("Drop");
        assert_eq!(effect.0, EFFECT_COPY);
        let IncomingDrop::Paths(paths) = probe.received.try_recv().expect("sink got the paths")
        else {
            panic!("file drop should keep the path");
        };
        assert_eq!(paths.len(), 1);
        assert!(same_file(&paths[0], &file));

        let mut effect = DROPEFFECT(EFFECT_MOVE);
        // SAFETY: 同上。
        unsafe {
            probe
                .target
                .Drop(&data, MODIFIERKEYS_FLAGS(0), point, &mut effect)
        }
        .expect("Drop with move only");
        assert_eq!(effect.0, EFFECT_NONE);
        assert!(
            probe.received.try_recv().is_err(),
            "a refused drop reaches no one"
        );
        assert!(file.is_file(), "the original file stays");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_float_window_accepts_anywhere_and_two_windows_do_not_share_state() {
        ensure_ole();
        let root = scratch("floats");
        let file = root.join("a.txt");
        std::fs::write(&file, b"a").expect("file");
        let data = data_object_for_paths(std::slice::from_ref(&file)).expect("data object");
        let all = EFFECT_COPY | EFFECT_MOVE | EFFECT_LINK;
        let first = probe();
        let second = probe();
        first.state.set_accepting(true);
        first.state.set_zone(DropZone::everywhere());
        assert_eq!(offer(&first, &data, 5000, all), EFFECT_COPY);
        assert_eq!(
            offer(&second, &data, 10, all),
            EFFECT_NONE,
            "the other window is closed"
        );

        let point = POINTL { x: 5000, y: 10 };
        let mut effect = DROPEFFECT(all);
        // SAFETY: data 是有效数据对象，point 和 effect 指向局部变量。
        unsafe {
            second
                .target
                .Drop(&data, MODIFIERKEYS_FLAGS(0), point, &mut effect)
        }
        .expect("Drop on the closed window");
        assert_eq!(effect.0, EFFECT_NONE);
        assert!(second.received.try_recv().is_err());
        assert!(
            first.received.try_recv().is_err(),
            "nothing was dropped on the first"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    use std::sync::atomic::AtomicUsize;

    use lanwork_core::shelves::{chrome_like_cf_html, descriptor_w, encode_utf16_text};
    use windows::Win32::Foundation::{
        DV_E_FORMATETC, DV_E_TYMED, E_INVALIDARG, E_NOTIMPL, OLE_E_ADVISENOTSUPPORTED, S_FALSE,
    };
    use windows::Win32::System::Com::{
        IAdviseSink, IDataObject_Impl, IEnumFORMATETC, IEnumFORMATETC_Impl, IEnumSTATDATA,
    };
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc};
    use windows::core::{BOOL, Ref};

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
                .unwrap_or_else(PoisonError::into_inner)
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
            if take == usize::try_from(celt).unwrap_or(usize::MAX) {
                S_OK
            } else {
                S_FALSE
            }
        }

        fn Skip(&self, celt: u32) -> windows::core::Result<()> {
            let cursor = self.cursor.load(Ordering::Relaxed);
            let next = cursor.saturating_add(usize::try_from(celt).unwrap_or(usize::MAX));
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

    #[test]
    fn url_drop_uses_the_html_href_and_the_descriptor_title_without_reading_file_contents() {
        ensure_ole();
        let ids = web_format_ids();
        let contents_id = register_clipboard_format("FileContents");
        let href = "https://github.com/example/page";
        let gets = Arc::new(Mutex::new(Vec::new()));
        let data: IDataObject = MemData {
            items: vec![
                (
                    ids.html,
                    chrome_like_cf_html("https://example.com/source", href, "链接文字"),
                ),
                (ids.url_w, encode_utf16_text("https://example.com/lock")),
                (
                    CF_UNICODETEXT.0,
                    encode_utf16_text("https://example.com/lock"),
                ),
                (ids.descriptor, descriptor_w(&["页面.url"])),
                (
                    contents_id,
                    b"[InternetShortcut]\r\nURL=https://secret.example/hidden\r\n".to_vec(),
                ),
                (CF_HDROP.0, b"C:\\should-not-read.url\0".to_vec()),
            ],
            gets: Arc::clone(&gets),
        }
        .into();
        let probe = probe();
        probe.state.set_accepting(true);
        let all = EFFECT_COPY | EFFECT_MOVE | EFFECT_LINK;
        assert_eq!(offer(&probe, &data, 10, all), EFFECT_COPY);
        assert!(
            gets.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty(),
            "DragEnter 只 QueryGetData，不 GetData"
        );

        let point = POINTL { x: 10, y: 10 };
        let mut effect = DROPEFFECT(all);
        unsafe {
            probe
                .target
                .Drop(&data, MODIFIERKEYS_FLAGS(0), point, &mut effect)
        }
        .expect("Drop");
        assert_eq!(effect.0, EFFECT_COPY);
        assert_eq!(
            probe.received.try_recv().expect("web sink"),
            IncomingDrop::Web {
                url: href.to_owned(),
                title: "页面".to_owned(),
            }
        );

        let mut effect = DROPEFFECT(EFFECT_MOVE);
        unsafe {
            probe
                .target
                .Drop(&data, MODIFIERKEYS_FLAGS(0), point, &mut effect)
        }
        .expect("move-only drop");
        assert_eq!(effect.0, EFFECT_NONE);
        assert!(probe.received.try_recv().is_err());

        let recorded = gets.lock().unwrap_or_else(PoisonError::into_inner).clone();
        assert!(
            recorded
                .iter()
                .all(|id| *id != contents_id && *id != CF_HDROP.0),
            "virtual file was read: {recorded:?}"
        );
    }

    #[test]
    fn descriptor_without_a_parsed_url_rejects_instead_of_reading_hdrop() {
        ensure_ole();
        let ids = web_format_ids();
        let contents_id = register_clipboard_format("FileContents");
        let gets = Arc::new(Mutex::new(Vec::new()));
        let data: IDataObject = MemData {
            items: vec![
                (ids.html, b"Version:0.9\r\nStartHTML:00000000\r\n".to_vec()),
                (ids.url_w, encode_utf16_text("not a url")),
                (
                    ids.moz,
                    encode_utf16_text("also not\nhttps://example.com/second"),
                ),
                (CF_UNICODETEXT.0, encode_utf16_text("C:\\virtual\\page.url")),
                (ids.descriptor, descriptor_w(&["page.url"])),
                (
                    contents_id,
                    b"[InternetShortcut]\r\nURL=https://secret.example/hidden\r\n".to_vec(),
                ),
                (CF_HDROP.0, b"C:\\virtual\\page.url\0".to_vec()),
            ],
            gets: Arc::clone(&gets),
        }
        .into();
        let probe = probe();
        probe.state.set_accepting(true);
        let all = EFFECT_COPY | EFFECT_MOVE | EFFECT_LINK;
        assert_eq!(offer(&probe, &data, 10, all), EFFECT_COPY);
        assert!(
            gets.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty(),
            "DragEnter 只 QueryGetData，不 GetData"
        );

        let point = POINTL { x: 10, y: 10 };
        let mut effect = DROPEFFECT(all);
        unsafe {
            probe
                .target
                .Drop(&data, MODIFIERKEYS_FLAGS(0), point, &mut effect)
        }
        .expect("Drop");
        assert_eq!(effect.0, EFFECT_NONE);
        assert!(
            probe.received.try_recv().is_err(),
            "解析不出网址时不把虚拟 .url 存成路径"
        );
        let recorded = gets.lock().unwrap_or_else(PoisonError::into_inner).clone();
        assert!(
            recorded
                .iter()
                .all(|id| *id != contents_id && *id != CF_HDROP.0),
            "rejected drop still read a virtual file: {recorded:?}"
        );
    }

    fn test_window() -> HWND {
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, WINDOW_EX_STYLE, WS_POPUP};
        use windows::core::w;
        // SAFETY: STATIC 是系统自带的窗口类，不需要实例句柄；参数都是常量，句柄由测试自己销毁。
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("lanwork-shelf-test"),
                WS_POPUP,
                0,
                0,
                10,
                10,
                None,
                None,
                None,
                None,
            )
        }
        .expect("test window")
    }

    #[test]
    fn each_window_registers_on_its_own_handle_and_revokes_when_its_registration_drops() {
        use windows::Win32::Foundation::DRAGDROP_E_NOTREGISTERED;
        use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;
        ensure_ole();
        let (first, second) = (test_window(), test_window());
        let sink: DropSink = Arc::new(|_| {});
        let registration_one =
            register(first, Arc::new(DropState::default()), Arc::clone(&sink)).expect("first");
        let registration_two =
            register(second, Arc::new(DropState::default()), sink).expect("second");

        drop(registration_one);
        // SAFETY: first 是本测试创建的窗口；已撤销时只返回错误。
        let again = unsafe { RevokeDragDrop(first) };
        assert_eq!(
            again.expect_err("already revoked").code(),
            DRAGDROP_E_NOTREGISTERED
        );
        // SAFETY: second 仍然登记着，撤销它不影响第一个窗口。
        unsafe { RevokeDragDrop(second) }.expect("the second window is still registered");

        drop(registration_two);
        // SAFETY: 两个窗口都由本测试创建，之后不再使用。
        unsafe {
            let _ = DestroyWindow(first);
            let _ = DestroyWindow(second);
        }
    }
}
