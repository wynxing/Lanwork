//! 收纳页的原生 OLE 拖入和拖出。判定规则在 `lanwork_core::panel`（`drop_effect`、`continue_drag` 等），
//! 这里只接 Win32。做法来自 `spikes/dnd`，那份记录见 `docs/roadmap.md`「外部拖放」。
//!
//! 放下目标注册在面板的窗口句柄上，所以只在收纳标签显示、指针在收纳页区域内、数据里有文件时才接受。
//! 允许的效果只有复制和快捷方式，不返回移动。只读路径，不打开、不复制、不移动、不删除用户文件。

use std::ffi::OsString;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use lanwork_core::panel::{
    DragContinue, DropZone, EFFECT_NONE, OUTGOING_EFFECTS, continue_drag, drop_effect,
};
use windows::Win32::Foundation::{
    DRAGDROP_E_ALREADYREGISTERED, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS,
    HWND, POINT, POINTL, S_OK,
};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{
    CoTaskMemFree, DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL,
};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DoDragDrop, IDropSource, IDropSource_Impl, IDropTarget, IDropTarget_Impl,
    OleInitialize, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop,
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

/// 放下目标读到文件路径之后调用。在界面线程上、OLE 的放下回调里执行，所以只应登记，不应做耗时的事。
pub(crate) type DropSink = Arc<dyn Fn(Vec<PathBuf>) + Send + Sync>;

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
        self.has_files.store(files, Ordering::Release);
        write_effect(effect, self.state.effect(point, files, read_effect(effect)));
        Ok(())
    }

    fn DragOver(
        &self,
        _keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let files = self.has_files.load(Ordering::Acquire);
        write_effect(effect, self.state.effect(point, files, read_effect(effect)));
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        self.has_files.store(false, Ordering::Release);
        Ok(())
    }

    fn Drop(
        &self,
        data: windows::core::Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        self.has_files.store(false, Ordering::Release);
        let files = data.as_ref().is_some_and(has_hdrop);
        let chosen = self.state.effect(point, files, read_effect(effect));
        write_effect(effect, chosen);
        if chosen == EFFECT_NONE {
            return Ok(());
        }
        let Some(data) = data.as_ref() else {
            write_effect(effect, EFFECT_NONE);
            return Ok(());
        };
        match paths_from_data_object(data) {
            Ok(paths) if !paths.is_empty() => (self.sink)(paths),
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
        received: mpsc::Receiver<Vec<PathBuf>>,
    }

    fn probe() -> Probe {
        let state = Arc::new(DropState::default());
        state.set_zone(open_zone());
        let (send, received) = mpsc::channel();
        let send = Mutex::new(send);
        let sink: DropSink = Arc::new(move |paths| {
            let _ = send
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send(paths);
        });
        let target: IDropTarget = ShelfDrop {
            state: Arc::clone(&state),
            sink,
            has_files: AtomicBool::new(false),
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
        let paths = probe.received.try_recv().expect("sink got the paths");
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
}
