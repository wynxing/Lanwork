//! 原生 OLE 拖入和拖出。判定规则在 `logic`，这里只接 Win32。

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{
    DRAGDROP_E_ALREADYREGISTERED, DRAGDROP_E_NOTREGISTERED, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP,
    DRAGDROP_S_USEDEFAULTCURSORS, HWND, POINT, POINTL, S_OK,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, DVASPECT_CONTENT, FORMATETC,
    IDataObject, IPersistFile, TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::GlobalFindAtomW;
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DoDragDrop, IDropSource, IDropSource_Impl, IDropTarget, IDropTarget_Impl,
    OleInitialize, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
};
use windows::Win32::System::SystemServices::MK_LBUTTON;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, DragQueryFileW, HDROP, ILFree, IShellItemArray, IShellLinkW,
    SHCreateShellItemArrayFromDataObject, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
    SIGDN_FILESYSPATH, ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::{GetPropW, GetSystemMetrics, SM_CXDRAG, SM_CYDRAG};
use windows::core::BOOL;
use windows::core::{HRESULT, Interface, PCWSTR, implement, w};

use crate::logic::{
    self, DragContinue, EFFECT_COPY, EFFECT_LINK, EFFECT_MOVE, EFFECT_NONE, Zone, effect_for_drop,
    query_continue,
};

pub struct SharedDrop {
    pub hwnd: AtomicIsize,
    pub zone: Mutex<Zone>,
    pub log: Mutex<String>,
}

impl SharedDrop {
    pub fn new() -> Self {
        Self {
            hwnd: AtomicIsize::new(0),
            zone: Mutex::new(Zone::default()),
            log: Mutex::new(String::new()),
        }
    }

    pub fn append_log(&self, line: &str) {
        let mut log = self.log.lock().expect("drop log");
        if !log.is_empty() {
            log.push('\n');
        }
        log.push_str(line);
    }
}

fn note(shared: &SharedDrop, line: &str) {
    println!("dnd: {line}");
    shared.append_log(line);
}

#[implement(IDropTarget)]
pub struct ShelfDrop {
    shared: std::sync::Arc<SharedDrop>,
}

impl ShelfDrop {
    pub fn new(shared: std::sync::Arc<SharedDrop>) -> Self {
        Self { shared }
    }
}

fn read_effect(slot: *mut DROPEFFECT) -> u32 {
    if slot.is_null() {
        EFFECT_NONE
    } else {
        unsafe { (*slot).0 }
    }
}

fn write_effect(slot: *mut DROPEFFECT, effect: u32) {
    if !slot.is_null() {
        unsafe { *slot = DROPEFFECT(effect) }
    }
}

fn client_from_screen(hwnd: HWND, pt: &POINTL) -> (i32, i32) {
    if hwnd.0.is_null() {
        return (pt.x, pt.y);
    }
    let mut client = POINT { x: pt.x, y: pt.y };
    let _ = unsafe { windows::Win32::Graphics::Gdi::ScreenToClient(hwnd, &mut client) };
    (client.x, client.y)
}

fn decide(shared: &SharedDrop, pt: &POINTL, offered: u32) -> u32 {
    let hwnd = HWND(shared.hwnd.load(Ordering::Acquire) as *mut std::ffi::c_void);
    let (x, y) = client_from_screen(hwnd, pt);
    let zone = *shared.zone.lock().expect("zone");
    effect_for_drop(logic::point_in_zone(zone, x, y), offered)
}

#[allow(non_snake_case)]
impl IDropTarget_Impl for ShelfDrop_Impl {
    fn DragEnter(
        &self,
        data: windows::core::Ref<IDataObject>,
        _keys: windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let offered = read_effect(effect);
        let accept = data.as_ref().is_some_and(has_hdrop);
        let chosen = if accept {
            decide(&self.shared, pt, offered)
        } else {
            EFFECT_NONE
        };
        write_effect(effect, chosen);
        Ok(())
    }

    fn DragOver(
        &self,
        _keys: windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let offered = read_effect(effect);
        write_effect(effect, decide(&self.shared, pt, offered));
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        Ok(())
    }

    fn Drop(
        &self,
        data: windows::core::Ref<IDataObject>,
        _keys: windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let offered = read_effect(effect);
        let chosen = decide(&self.shared, pt, offered);
        write_effect(effect, chosen);
        if chosen == EFFECT_NONE {
            note(&self.shared, "drop rejected");
            return Ok(());
        }
        let Some(data) = data.as_ref() else {
            note(&self.shared, "drop without data object");
            write_effect(effect, EFFECT_NONE);
            return Ok(());
        };
        match paths_from_data_object(data) {
            Ok(paths) => {
                let rendered = paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(" | ");
                note(
                    &self.shared,
                    &format!("drop effect={chosen} paths={rendered}"),
                );
            }
            Err(error) => note(&self.shared, &format!("drop read failed: {error}")),
        }
        Ok(())
    }
}

pub fn has_hdrop(data: &IDataObject) -> bool {
    let format = hdrop_format();
    unsafe { data.QueryGetData(&format) }.is_ok()
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

pub fn paths_from_data_object(data: &IDataObject) -> Result<Vec<PathBuf>, String> {
    let format = hdrop_format();
    match unsafe { data.GetData(&format) } {
        Ok(mut medium) => {
            let hdrop = HDROP(unsafe { medium.u.hGlobal }.0);
            let paths = paths_from_hdrop(hdrop);
            unsafe { ReleaseStgMedium(&mut medium) };
            Ok(paths)
        }
        // The shell builds CF_HDROP with SHGetPathFromIDListW, whose binding is a
        // fixed 260 UTF-16 units. A longer path fails here, before any HDROP exists,
        // so DragQueryFileW never gets a chance to size its own buffer.
        Err(error) if error.code().0 == 0x8007_007A_u32 as i32 => paths_from_shell_items(data)
            .map_err(|fallback| {
                format!("GetData CF_HDROP: {error}; shell-item fallback: {fallback}")
            }),
        Err(error) => Err(format!("GetData CF_HDROP: {error}")),
    }
}

fn paths_from_shell_items(data: &IDataObject) -> Result<Vec<PathBuf>, String> {
    let items: IShellItemArray = unsafe { SHCreateShellItemArrayFromDataObject(data) }
        .map_err(|error| format!("SHCreateShellItemArrayFromDataObject: {error}"))?;
    let count = unsafe { items.GetCount() }.map_err(|error| format!("GetCount: {error}"))?;
    let mut paths = Vec::with_capacity(count as usize);
    for index in 0..count {
        let item =
            unsafe { items.GetItemAt(index) }.map_err(|error| format!("GetItemAt: {error}"))?;
        let name = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }
            .map_err(|error| format!("GetDisplayName: {error}"))?;
        let path = path_from_pwstr(name);
        unsafe { CoTaskMemFree(Some(name.0.cast())) };
        if path.as_os_str().is_empty() {
            return Err(format!("shell item {index} had an empty path"));
        }
        paths.push(path);
    }
    Ok(paths)
}

fn path_from_pwstr(name: windows::core::PWSTR) -> PathBuf {
    let wide = unsafe {
        if name.0.is_null() {
            return PathBuf::new();
        }
        let mut len = 0usize;
        while *name.0.add(len) != 0 {
            len += 1;
        }
        std::slice::from_raw_parts(name.0, len)
    };
    let mut path = PathBuf::from(OsString::from_wide(wide));
    let encoded: Vec<u16> = path.as_os_str().encode_wide().collect();
    const VERBATIM: [u16; 4] = [b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16];
    if encoded.starts_with(&VERBATIM) {
        path = PathBuf::from(OsString::from_wide(&encoded[VERBATIM.len()..]));
    }
    path
}

pub fn paths_from_hdrop(hdrop: HDROP) -> Vec<PathBuf> {
    let count = unsafe { DragQueryFileW(hdrop, u32::MAX, None) };
    let mut paths = Vec::with_capacity(count as usize);
    for index in 0..count {
        let len = unsafe { DragQueryFileW(hdrop, index, None) };
        let mut buf = vec![0u16; len as usize + 1];
        let copied = unsafe { DragQueryFileW(hdrop, index, Some(&mut buf)) };
        let end = (copied as usize).min(buf.len());
        let end = buf[..end].iter().position(|unit| *unit == 0).unwrap_or(end);
        paths.push(PathBuf::from(OsString::from_wide(&buf[..end])));
    }
    paths
}

pub fn hdrop_from_paths(paths: &[PathBuf]) -> Result<HDROP, String> {
    let mut wide = Vec::<u16>::new();
    for path in paths {
        wide.extend(path.as_os_str().encode_wide());
        wide.push(0);
    }
    wide.push(0);
    let header = 20usize;
    let bytes = header + wide.len() * 2;
    unsafe {
        let global = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|error| error.to_string())?;
        let ptr = GlobalLock(global);
        if ptr.is_null() {
            return Err("GlobalLock failed".to_string());
        }
        let base = ptr.cast::<u8>();
        std::ptr::write_bytes(base, 0, header);
        base.cast::<u32>().write(header as u32);
        base.add(16).cast::<i32>().write(1);
        std::ptr::copy_nonoverlapping(wide.as_ptr(), base.add(header).cast::<u16>(), wide.len());
        let _ = GlobalUnlock(global);
        Ok(HDROP(global.0))
    }
}

pub fn wide_len(path: &Path) -> usize {
    path.as_os_str().encode_wide().count()
}

struct ParsedId {
    pidl: *mut ITEMIDLIST,
}

impl Drop for ParsedId {
    fn drop(&mut self) {
        if !self.pidl.is_null() {
            unsafe { ILFree(Some(self.pidl.cast())) };
        }
    }
}

pub fn data_object_for_paths(paths: &[PathBuf]) -> Result<IDataObject, String> {
    let mut pidls = Vec::with_capacity(paths.len());
    for path in paths {
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut pidl = std::ptr::null_mut();
        unsafe {
            SHParseDisplayName(
                PCWSTR(wide.as_ptr()),
                None::<&windows::Win32::System::Com::IBindCtx>,
                &mut pidl,
                0,
                None,
            )
        }
        .map_err(|error| format!("SHParseDisplayName {}: {error}", path.display()))?;
        pidls.push(ParsedId { pidl });
    }
    let raw: Vec<*const ITEMIDLIST> = pidls.iter().map(|item| item.pidl.cast_const()).collect();
    let items: IShellItemArray = unsafe { SHCreateShellItemArrayFromIDLists(&raw) }
        .map_err(|error| format!("SHCreateShellItemArrayFromIDLists: {error}"))?;
    let data = unsafe {
        items.BindToHandler::<_, IDataObject>(
            None::<&windows::Win32::System::Com::IBindCtx>,
            &BHID_DataObject,
        )
    }
    .map_err(|error| format!("BindToHandler BHID_DataObject: {error}"))?;
    Ok(data)
}

#[implement(IDropSource)]
pub struct ShelfSource {
    pub force_cancel: bool,
}

#[allow(non_snake_case)]
impl IDropSource_Impl for ShelfSource_Impl {
    fn QueryContinueDrag(
        &self,
        escape: BOOL,
        keys: windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS,
    ) -> HRESULT {
        let left = keys.0 & MK_LBUTTON.0 != 0;
        match query_continue(escape.as_bool(), left, self.force_cancel) {
            DragContinue::Continue => S_OK,
            DragContinue::Drop => DRAGDROP_S_DROP,
            DragContinue::Cancel => DRAGDROP_S_CANCEL,
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

pub struct DragOutcome {
    pub code: i32,
    pub effect: u32,
}

pub fn drag_out(paths: &[PathBuf], force_cancel: bool) -> Result<DragOutcome, String> {
    let data = data_object_for_paths(paths)?;
    let source: IDropSource = ShelfSource { force_cancel }.into();
    let mut effect = DROPEFFECT(EFFECT_NONE);
    let code = unsafe {
        DoDragDrop(
            &data,
            &source,
            DROPEFFECT(EFFECT_COPY | EFFECT_LINK),
            &mut effect,
        )
    };
    Ok(DragOutcome {
        code: code.0,
        effect: effect.0,
    })
}

pub fn ole_initialize() -> Result<(), String> {
    unsafe { OleInitialize(None) }.map_err(|error| error.to_string())
}

pub fn apartment() -> String {
    let mut kind = windows::Win32::System::Com::APTTYPE::default();
    let mut qualifier = windows::Win32::System::Com::APTTYPEQUALIFIER::default();
    match unsafe { windows::Win32::System::Com::CoGetApartmentType(&mut kind, &mut qualifier) } {
        Ok(()) => format!("apartment={kind:?} qualifier={qualifier:?}"),
        Err(error) => format!("apartment-error={error}"),
    }
}

pub enum DropRegistration {
    RegisteredByUs,
    AlreadyRegistered,
    NotRegistered,
    Error(String),
}

pub fn probe_drop_registration(hwnd: HWND, probe: &IDropTarget) -> DropRegistration {
    match unsafe { RegisterDragDrop(hwnd, probe) } {
        Ok(()) => {
            let _ = unsafe { RevokeDragDrop(hwnd) };
            DropRegistration::RegisteredByUs
        }
        Err(error) if error.code() == DRAGDROP_E_ALREADYREGISTERED => {
            DropRegistration::AlreadyRegistered
        }
        Err(error) if error.code() == DRAGDROP_E_NOTREGISTERED => DropRegistration::NotRegistered,
        Err(error) => DropRegistration::Error(error.to_string()),
    }
}

pub fn replace_drop_target(hwnd: HWND, target: &IDropTarget) -> Result<&'static str, String> {
    match unsafe { RegisterDragDrop(hwnd, target) } {
        Ok(()) => Ok("RegisterDragDrop"),
        Err(error) if error.code() == DRAGDROP_E_ALREADYREGISTERED => {
            unsafe { RevokeDragDrop(hwnd) }.map_err(|error| format!("RevokeDragDrop: {error}"))?;
            unsafe { RegisterDragDrop(hwnd, target) }
                .map_err(|error| format!("RegisterDragDrop after revoke: {error}"))?;
            Ok("RevokeDragDrop then RegisterDragDrop")
        }
        Err(error) => Err(error.to_string()),
    }
}

pub fn registered_drop_pointer(hwnd: HWND) -> Option<isize> {
    unsafe {
        let atom = GlobalFindAtomW(w!("OleDropTargetInterface"));
        if atom == 0 {
            return None;
        }
        let prop = GetPropW(hwnd, PCWSTR(atom as usize as *const u16));
        if prop.0.is_null() {
            None
        } else {
            Some(prop.0 as isize)
        }
    }
}

pub fn com_pointer(target: &IDropTarget) -> isize {
    windows::core::Interface::as_raw(target) as isize
}

pub fn drag_metrics() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) }
}

pub fn handle_snapshot() -> String {
    use windows::Win32::System::Threading::{
        GR_GDIOBJECTS, GR_USEROBJECTS, GetCurrentProcess, GetGuiResources, GetProcessHandleCount,
    };
    let mut handles = 0u32;
    let handle_result = unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut handles) };
    let gdi = unsafe { GetGuiResources(GetCurrentProcess(), GR_GDIOBJECTS) };
    let user = unsafe { GetGuiResources(GetCurrentProcess(), GR_USEROBJECTS) };
    format!(
        "handles={handles} handle-ok={} gdi={gdi} user={user}",
        handle_result.is_ok()
    )
}

static OLE_READY: OnceLock<Result<(), String>> = OnceLock::new();

pub fn ensure_ole() -> Result<(), String> {
    OLE_READY.get_or_init(ole_initialize).clone()
}

pub fn create_shortcut(link: &Path, target: &Path) -> Result<(), String> {
    ensure_ole()?;
    let shell: IShellLinkW = unsafe {
        CoCreateInstance(
            &ShellLink,
            None::<&windows::core::IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }
    .map_err(|error| format!("CoCreateInstance ShellLink: {error}"))?;
    let target_wide: Vec<u16> = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe { shell.SetPath(PCWSTR(target_wide.as_ptr())) }
        .map_err(|error| format!("IShellLink::SetPath: {error}"))?;
    let file: IPersistFile = shell.cast().map_err(|error| error.to_string())?;
    let link_wide: Vec<u16> = link
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe { file.Save(PCWSTR(link_wide.as_ptr()), true) }
        .map_err(|error| format!("IPersistFile::Save: {error}"))?;
    Ok(())
}

pub fn os_version() -> String {
    use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(info: *mut OSVERSIONINFOW) -> i32;
    }
    let mut info = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..OSVERSIONINFOW::default()
    };
    let status = unsafe { RtlGetVersion(&mut info) };
    format!(
        "{}.{}.{} status={status}",
        info.dwMajorVersion, info.dwMinorVersion, info.dwBuildNumber
    )
}

pub const DRAG_CANCEL_CODE: i32 = DRAGDROP_S_CANCEL.0;

pub fn effect_name(effect: u32) -> &'static str {
    if effect == EFFECT_NONE {
        "none"
    } else if effect == EFFECT_COPY {
        "copy"
    } else if effect == EFFECT_MOVE {
        "move"
    } else if effect == EFFECT_LINK {
        "link"
    } else if effect == EFFECT_COPY | EFFECT_LINK {
        "copy|link"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_effect_constants_match_logic() {
        use windows::Win32::System::Ole::{
            DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
        };
        assert_eq!(DROPEFFECT_NONE.0, EFFECT_NONE);
        assert_eq!(DROPEFFECT_COPY.0, EFFECT_COPY);
        assert_eq!(DROPEFFECT_MOVE.0, EFFECT_MOVE);
        assert_eq!(DROPEFFECT_LINK.0, EFFECT_LINK);
    }

    #[test]
    fn synthetic_hdrop_keeps_lnk_and_long_path() {
        let lnk = PathBuf::from(r"C:\temp\shortcut.lnk");
        let mut long = PathBuf::from(r"C:\lanwork-dnd");
        while wide_len(&long) <= 260 {
            long.push("segment-of-forty-chars-for-the-path");
        }
        let file = long.join("long.txt");
        assert!(wide_len(&file) > 260, "wide len {}", wide_len(&file));
        let folder = PathBuf::from(r"C:\temp\folder");
        let hdrop = hdrop_from_paths(&[lnk.clone(), file.clone(), folder.clone()]).expect("hdrop");
        let paths = paths_from_hdrop(hdrop);
        unsafe { windows::Win32::UI::Shell::DragFinish(hdrop) };
        assert_eq!(paths, vec![lnk, file, folder]);
        assert!(paths[0].extension().is_some_and(|ext| ext == "lnk"));
        assert!(wide_len(&paths[1]) > 260);
    }

    #[test]
    fn shell_data_object_keeps_long_path_and_lnk() {
        ensure_ole().expect("ole");
        let root =
            std::env::temp_dir().join(format!("lanwork-dnd-longread-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("root");
        let readme = root.join("readme.txt");
        std::fs::write(&readme, b"readme").expect("readme");
        let shortcut = root.join("shortcut.lnk");
        create_shortcut(&shortcut, &readme).expect("shortcut");

        let mut nested = root.clone();
        let piece = "d".repeat(40);
        loop {
            if wide_len(&nested) > 230 {
                break;
            }
            nested.push(&piece);
        }
        std::fs::create_dir_all(&nested).expect("long dir");
        let long_file = nested.join("long.txt");
        std::fs::write(&long_file, b"long").expect("long file");
        assert!(wide_len(&long_file) > 260, "{}", wide_len(&long_file));

        let direct = data_object_for_paths(std::slice::from_ref(&long_file)).expect("data");
        let hdrop_code = match unsafe { direct.GetData(&hdrop_format()) } {
            Ok(mut medium) => {
                unsafe { ReleaseStgMedium(&mut medium) };
                None
            }
            Err(error) => Some(error.code().0),
        };
        assert_eq!(
            hdrop_code,
            Some(0x8007_007A_u32 as i32),
            "CF_HDROP GetData did not return 0x8007007A"
        );
        let paths = paths_from_data_object(&direct).expect("long read");
        assert_eq!(paths, vec![long_file.clone()]);
        assert!(wide_len(&paths[0]) > 260);

        let link_data = data_object_for_paths(std::slice::from_ref(&shortcut)).expect("link data");
        let link_paths = paths_from_shell_items(&link_data).expect("link names");
        assert_eq!(link_paths, vec![shortcut]);

        let _ = std::fs::remove_dir_all(&root);
    }
}
