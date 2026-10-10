#![cfg_attr(not(windows), allow(dead_code))]

//! 拖动跨 DPI 时保住窗口逻辑尺寸，并在松手后重新打开输入法候选窗。
//!
//! winit 0.30.13 在 `WM_DPICHANGED` 里自己算外框，并用 `MonitorFromWindow`
//! 判断目标显示器。拖动过程中这个调用仍返回正在离开的显示器，于是窗口被推回去，
//! 再收到一次 DPI 变化。从 150% 回到 100% 时，第二次计算仍拿着旧的物理像素，
//! 窗口会变大。Slint 1.18.1 收到缩放变化后不调用 `set_ime_cursor_area`。
//!
//! 外框按系统建议矩形保住之后，客户区坐标已经随 DPI 更新，候选窗仍会消失。
//! 微软拼音走 TSF。`ImmSetCandidateWindow` 只改位置记录，不重新打开已经关掉的候选 UI。
//! 拖动还没结束时窗口跨在两块屏幕上，这时改位置会把候选 UI 关掉。
//!
//! 绕过只在这个 spike 里：拖动期间把 `WM_WINDOWPOSCHANGING` 改成系统建议矩形，
//! 并且先不碰输入法。松手后按该 DPI 收外框，用逻辑光标乘 `dpi/96` 重设组合窗和候选窗，
//! 再通知 TSF 布局变了。上游是 winit 0.30.13（`i-slint-backend-winit` 1.18.1 带进来的）
//! 和 Slint 1.18.1。

use std::cell::RefCell;

/// 与 `ui/main.slint` 里 `MainWindow` 的 `width` / `height` 相同。
pub const LOGICAL_WIDTH: f64 = 720.0;
pub const LOGICAL_HEIGHT: f64 = 760.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OuterRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl OuterRect {
    pub fn width(self) -> i32 {
        self.right - self.left
    }

    pub fn height(self) -> i32 {
        self.bottom - self.top
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GoodSize {
    pub dpi: u32,
    pub width: i32,
    pub height: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PosOverride {
    None,
    /// 这次 `WM_DPICHANGED` 的建议矩形，位置和大小都用它。
    Full(OuterRect),
    /// 拖动还没结束时，只保住已经记下的外框大小，位置仍跟光标走。
    Size(i32, i32),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DragFix {
    pub dragging: bool,
    pub depth: u32,
    pub locked: Option<OuterRect>,
    pub good: Option<GoodSize>,
}

impl DragFix {
    pub fn on_enter_size_move(&mut self) {
        self.dragging = true;
    }

    pub fn on_exit_size_move(&mut self) {
        self.dragging = false;
    }

    /// 最外层、并且正在拖动、建议矩形有效时返回 true。嵌套的 DPI 消息不换掉第一张矩形。
    pub fn begin_dpi(&mut self, dpi: u32, suggested: OuterRect) -> bool {
        let outer = self.depth == 0;
        self.depth = self.depth.saturating_add(1);
        if outer && self.dragging && suggested.width() > 0 && suggested.height() > 0 {
            self.locked = Some(suggested);
            self.good = Some(GoodSize {
                dpi,
                width: suggested.width(),
                height: suggested.height(),
            });
            true
        } else {
            false
        }
    }

    pub fn end_dpi(&mut self) {
        self.depth = self.depth.saturating_sub(1);
        if self.depth == 0 {
            self.locked = None;
        }
    }

    pub fn pos_override(&self, dpi_now: u32) -> PosOverride {
        if let Some(rect) = self.locked {
            return PosOverride::Full(rect);
        }
        if self.dragging
            && let Some(good) = self.good
            && good.dpi == dpi_now
            && good.width > 0
            && good.height > 0
        {
            return PosOverride::Size(good.width, good.height);
        }
        PosOverride::None
    }

    pub fn expected_size(&self, dpi: u32) -> (i32, i32) {
        if let Some(good) = self.good
            && good.dpi == dpi
            && good.width > 0
            && good.height > 0
        {
            return (good.width, good.height);
        }
        (
            round_physical(LOGICAL_WIDTH, dpi),
            round_physical(LOGICAL_HEIGHT, dpi),
        )
    }
}

/// 与 winit `to_physical` 一样，四舍五入到整数像素。
pub fn round_physical(logical: f64, dpi: u32) -> i32 {
    (logical * f64::from(dpi) / 96.0).round() as i32
}

/// winit 0.30.13 用旧物理尺寸乘新缩放、除旧缩放，不用建议矩形。
#[cfg(test)]
fn winit_physical_after_scale(old_physical: u32, old_dpi: u32, new_dpi: u32) -> u32 {
    let logical = f64::from(old_physical) * 96.0 / f64::from(old_dpi);
    (logical * f64::from(new_dpi) / 96.0).round() as u32
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaretPx {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

pub fn caret_at_dpi(x: f64, y: f64, w: f64, h: f64, dpi: u32) -> CaretPx {
    CaretPx {
        x: round_physical(x, dpi),
        y: round_physical(y, dpi),
        width: round_physical(w, dpi).max(1),
        height: round_physical(h, dpi).max(1),
    }
}

pub fn size_mismatch(actual_w: i32, actual_h: i32, expected_w: i32, expected_h: i32) -> bool {
    (actual_w - expected_w).abs() > 1 || (actual_h - expected_h).abs() > 1
}

/// 与 `windows` 0.62 里 `CFS_*` 的数值相同。Win32 头文件里的值。
pub const STYLE_POINT: u32 = 0x0002;
pub const STYLE_FORCE_POSITION: u32 = 0x0020;
pub const STYLE_CANDIDATEPOS: u32 = 0x0040;
pub const STYLE_EXCLUDE: u32 = 0x0080;

/// 客户区里要写给 IMM 的两份位置。坐标都是客户区像素。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImeForms {
    pub composition_style: u32,
    pub composition_x: i32,
    pub composition_y: i32,
    pub exclude_style: u32,
    pub area_left: i32,
    pub area_top: i32,
    pub area_right: i32,
    pub area_bottom: i32,
    pub candidate_style: u32,
    pub candidate_x: i32,
    pub candidate_y: i32,
}

/// 组合窗用 `CFS_POINT | CFS_FORCE_POSITION`，点在光标下沿，和 winit 的 y 相同，并强制使用该点。
/// 候选窗先按 `CFS_CANDIDATEPOS` 放在光标下沿，再写 `CFS_EXCLUDE`，最后留下的记录与 winit 一样。
/// `CFS_EXCLUDE` 的矩形只包住光标。
pub fn ime_forms(caret: CaretPx) -> ImeForms {
    let bottom = caret.y.saturating_add(caret.height);
    let right = caret.x.saturating_add(caret.width);
    ImeForms {
        composition_style: STYLE_POINT | STYLE_FORCE_POSITION,
        composition_x: caret.x,
        composition_y: bottom,
        exclude_style: STYLE_EXCLUDE,
        area_left: caret.x,
        area_top: caret.y,
        area_right: right,
        area_bottom: bottom,
        candidate_style: STYLE_CANDIDATEPOS,
        candidate_x: caret.x,
        candidate_y: bottom,
    }
}

/// 拖动还在进行时，这次 DPI 消息只改外框。松手或非拖动的 DPI 变化才重设输入法。
pub fn refresh_ime_on_this_dpi_message(dragging: bool) -> bool {
    !dragging
}

#[derive(Clone, Copy, Debug)]
struct LogicalCaret {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

type Logger = Box<dyn Fn(&str)>;

thread_local! {
    static FIX: RefCell<DragFix> = const { RefCell::new(DragFix {
        dragging: false,
        depth: 0,
        locked: None,
        good: None,
    }) };
    static CARET: RefCell<Option<LogicalCaret>> = const { RefCell::new(None) };
    static LOGGER: RefCell<Option<Logger>> = const { RefCell::new(None) };
    static PENDING_LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

fn with_fix<T>(f: impl FnOnce(&mut DragFix) -> T) -> T {
    FIX.with(|cell| f(&mut cell.borrow_mut()))
}

pub fn set_logger(logger: impl Fn(&str) + 'static) {
    LOGGER.with(|cell| {
        *cell.borrow_mut() = Some(Box::new(logger));
    });
}

pub fn set_logical_caret(x: f32, y: f32, w: f32, h: f32) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    CARET.with(|cell| {
        *cell.borrow_mut() = Some(LogicalCaret {
            x: f64::from(x),
            y: f64::from(y),
            w: f64::from(w),
            h: f64::from(h),
        });
    });
}

fn logical_caret() -> Option<LogicalCaret> {
    CARET.with(|cell| *cell.borrow())
}

fn note(line: &str) {
    eprintln!("{line}");
    PENDING_LOG.with(|cell| cell.borrow_mut().push(line.to_string()));
}

fn flush_notes() {
    let lines = PENDING_LOG.with(|cell| std::mem::take(&mut *cell.borrow_mut()));
    if lines.is_empty() {
        return;
    }
    LOGGER.with(|cell| {
        if let Some(logger) = cell.borrow().as_ref() {
            for line in &lines {
                logger(line);
            }
        }
    });
}

/// 装上子类时返回 true。非 Windows 返回 false，调用方不必记日志。
pub fn install(window: &slint::Window) -> Result<bool, String> {
    #[cfg(windows)]
    {
        win::install(window)
    }
    #[cfg(not(windows))]
    {
        let _ = window;
        Ok(false)
    }
}

#[cfg(windows)]
mod win {
    use super::{
        CaretPx, OuterRect, caret_at_dpi, flush_notes, ime_forms, logical_caret, note,
        refresh_ime_on_this_dpi_message, size_mismatch, with_fix,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::mem::{size_of, size_of_val};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::ClientToScreen;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::Input::Ime::{
        CANDIDATEFORM, COMPOSITIONFORM, ImmGetContext, ImmReleaseContext, ImmSetCandidateWindow,
        ImmSetCompositionWindow,
    };
    use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
    use windows::Win32::UI::TextServices::{
        CLSID_TF_ThreadMgr, IEnumTfUIElements, ITfCandidateListUIElement, ITfContextOwnerServices,
        ITfThreadMgr, ITfUIElementMgr,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateCaret, GetWindowRect, SWP_NOACTIVATE, SWP_NOCOPYBITS, SWP_NOMOVE, SWP_NOSIZE,
        SWP_NOZORDER, SetCaretPos, SetWindowPos, WINDOWPOS, WM_DPICHANGED, WM_ENTERSIZEMOVE,
        WM_EXITSIZEMOVE, WM_NCDESTROY, WM_WINDOWPOSCHANGING,
    };
    use windows::core::{Interface, s, w};

    const SUBCLASS_ID: usize = 1;

    thread_local! {
        static HWND_SLOT: Cell<Option<HWND>> = const { Cell::new(None) };
        static SETTLE_QUEUED: Cell<bool> = const { Cell::new(false) };
    }

    pub fn install(window: &slint::Window) -> Result<bool, String> {
        let hwnd = hwnd_of(window)?;
        // SAFETY: hwnd 来自当前 Slint 窗口。子类过程活得和进程一样久，id 只给这一次安装。
        let ok = unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, 0) };
        if !ok.as_bool() {
            return Err("SetWindowSubclass 返回 false".to_string());
        }
        HWND_SLOT.with(|slot| slot.set(Some(hwnd)));
        Ok(true)
    }

    fn hwnd_of(window: &slint::Window) -> Result<HWND, String> {
        let owned = window.window_handle();
        let handle = owned.window_handle().map_err(|error| error.to_string())?;
        match handle.as_raw() {
            RawWindowHandle::Win32(raw) => Ok(HWND(raw.hwnd.get() as *mut std::ffi::c_void)),
            other => Err(format!("窗口句柄不是 Win32: {other:?}")),
        }
    }

    fn dpi_of(hwnd: HWND) -> Option<u32> {
        // SAFETY: hwnd 是本 spike 的顶层窗口。失败时返回 0。
        let dpi = unsafe { GetDpiForWindow(hwnd) };
        (dpi > 0).then_some(dpi)
    }

    fn outer_of(hwnd: HWND) -> Option<OuterRect> {
        let mut rect = RECT::default();
        // SAFETY: rect 是本函数的局部变量。hwnd 是本窗口。
        unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
        Some(OuterRect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        })
    }

    fn place(hwnd: HWND, rect: OuterRect) {
        let width = rect.width();
        let height = rect.height();
        if width <= 0 || height <= 0 {
            return;
        }
        // SAFETY: hwnd 是本窗口。SWP_NOZORDER 时不使用插入位置。SWP_NOACTIVATE 避免输入法失焦。
        let _ = unsafe {
            SetWindowPos(
                hwnd,
                None,
                rect.left,
                rect.top,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOCOPYBITS,
            )
        };
    }

    fn suggested_rect(lparam: LPARAM) -> Option<OuterRect> {
        if lparam.0 == 0 {
            return None;
        }
        // SAFETY: WM_DPICHANGED 的 lParam 在这次消息返回前指向系统给出的 RECT。
        let rect = unsafe { *(lparam.0 as *const RECT) };
        let outer = OuterRect {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        };
        (outer.width() > 0 && outer.height() > 0).then_some(outer)
    }

    fn rewrite_pos(lparam: LPARAM, dpi_now: u32) {
        if lparam.0 == 0 {
            return;
        }
        let over = with_fix(|fix| fix.pos_override(dpi_now));
        match over {
            super::PosOverride::None => {}
            super::PosOverride::Full(rect) => {
                // SAFETY: WM_WINDOWPOSCHANGING 的 lParam 指向可写的 WINDOWPOS，直到本消息返回。
                let pos = unsafe { &mut *(lparam.0 as *mut WINDOWPOS) };
                pos.x = rect.left;
                pos.y = rect.top;
                pos.cx = rect.width();
                pos.cy = rect.height();
                pos.flags &= !(SWP_NOSIZE | SWP_NOMOVE);
            }
            super::PosOverride::Size(width, height) => {
                // SAFETY: 同上，只改大小，留下系统正在更新的位置。
                let pos = unsafe { &mut *(lparam.0 as *mut WINDOWPOS) };
                if pos.cx > 0 && pos.cy > 0 {
                    pos.cx = width;
                    pos.cy = height;
                    pos.flags &= !SWP_NOSIZE;
                }
            }
        }
    }

    fn refresh_ime(hwnd: HWND, dpi: u32) {
        let Some(caret) = logical_caret() else {
            return;
        };
        let px: CaretPx = caret_at_dpi(caret.x, caret.y, caret.w, caret.h, dpi);
        let forms = ime_forms(px);
        let area = RECT {
            left: forms.area_left,
            top: forms.area_top,
            right: forms.area_right,
            bottom: forms.area_bottom,
        };
        // 先放系统光标。部分路径在收到位置通知时读 GetCaretPos，那时客户区坐标要已经是新的。
        let caret_note = place_system_caret(hwnd, px);
        // SAFETY: ImmGetContext 与下面的 ImmReleaseContext 成对。HIMC 在 windows 0.62 里是 Copy，
        // 它的 Free 会调用 ImmDestroyContext。这里不要 free，只 ImmReleaseContext。
        let himc = unsafe { ImmGetContext(hwnd) };
        if himc.is_invalid() {
            note(&format!(
                "IME 光标 dpi={dpi} 客户区 {x},{y} {w}x{h} 没有 HIMC {caret_note}",
                x = px.x,
                y = px.y,
                w = px.width,
                h = px.height,
            ));
            return;
        }
        let composition = COMPOSITIONFORM {
            dwStyle: forms.composition_style,
            ptCurrentPos: POINT {
                x: forms.composition_x,
                y: forms.composition_y,
            },
            rcArea: area,
        };
        let candidate_pos = CANDIDATEFORM {
            dwIndex: 0,
            dwStyle: forms.candidate_style,
            ptCurrentPos: POINT {
                x: forms.candidate_x,
                y: forms.candidate_y,
            },
            rcArea: area,
        };
        let exclude = CANDIDATEFORM {
            dwIndex: 0,
            dwStyle: forms.exclude_style,
            ptCurrentPos: POINT { x: px.x, y: px.y },
            rcArea: area,
        };
        // SAFETY: himc 刚从本窗口取得。结构体都是局部变量，调用期间有效。
        // 先写 CFS_CANDIDATEPOS，再写 CFS_EXCLUDE，最后留下的记录与 winit 相同。
        // 不调用 ImmNotifyIME：CPS_CANCEL 和 CPS_COMPLETE 会清掉还没上屏的组合。
        unsafe {
            let _ = ImmSetCompositionWindow(himc, &composition);
            let _ = ImmSetCandidateWindow(himc, &candidate_pos);
            let _ = ImmSetCandidateWindow(himc, &exclude);
            let _ = ImmReleaseContext(hwnd, himc);
        }
        let screen = screen_point(hwnd, px.x, px.y);
        let layout = reopen_candidate();
        note(&format!(
            "IME 光标 dpi={dpi} 客户区 {x},{y} {w}x{h} 屏幕 {screen} {layout} {caret_note}",
            x = px.x,
            y = px.y,
            w = px.width,
            h = px.height,
        ));
    }

    fn screen_point(hwnd: HWND, x: i32, y: i32) -> String {
        let mut point = POINT { x, y };
        // SAFETY: point 是局部变量。hwnd 是本窗口。传入的是客户区坐标。
        let ok = unsafe { ClientToScreen(hwnd, &mut point) };
        if ok.as_bool() {
            format!("{},{}", point.x, point.y)
        } else {
            "未知".to_string()
        }
    }

    fn place_system_caret(hwnd: HWND, caret: CaretPx) -> &'static str {
        // SAFETY: hwnd 是本窗口。None 用系统默认光标形状。不调用 ShowCaret，避免在 Slint 光标上再画一条。
        // 微软拼音在部分路径上用 GetCaretPos，而不是只看候选窗记录。
        let created = unsafe { CreateCaret(hwnd, None, caret.width.max(1), caret.height.max(1)) };
        if created.is_err() {
            return "系统光标 失败";
        }
        // SAFETY: 光标属于本窗口。坐标是客户区像素，取光标左上角。
        if unsafe { SetCaretPos(caret.x, caret.y) }.is_err() {
            return "系统光标 失败";
        }
        "系统光标 已设"
    }

    fn reopen_candidate() -> String {
        let mgr = match thread_mgr() {
            Ok(mgr) => mgr,
            Err(error) => return format!("布局通知 失败 {error}"),
        };
        let layout = match notify_layout(&mgr) {
            Ok(()) => "布局通知 成功".to_string(),
            Err(error) => format!("布局通知 失败 {error}"),
        };
        let shown = match show_candidate_elements(&mgr) {
            Ok(count) => format!("候选元素 {count}"),
            Err(error) => format!("候选元素 失败 {error}"),
        };
        format!("{layout} {shown}")
    }

    fn thread_mgr() -> Result<ITfThreadMgr, String> {
        match thread_mgr_from_msctf() {
            Ok(mgr) => Ok(mgr),
            Err(first) => co_create_thread_mgr().map_err(|second| format!("{first}；{second}")),
        }
    }

    fn thread_mgr_from_msctf() -> Result<ITfThreadMgr, String> {
        // SAFETY: msctf.dll 是系统库。GetModuleHandleW 不增加引用计数。名字以 0 结尾。
        let module =
            unsafe { GetModuleHandleW(w!("msctf.dll")) }.map_err(|_| "msctf 未加载".to_string())?;
        // SAFETY: 模块句柄仍然有效。s! 是静态 C 字符串。
        let proc = unsafe { GetProcAddress(module, s!("TF_GetThreadMgr")) }
            .ok_or_else(|| "msctf 没有 TF_GetThreadMgr".to_string())?;
        type GetMgr = unsafe extern "system" fn(*mut *mut c_void) -> windows::core::HRESULT;
        if size_of::<GetMgr>() != size_of_val(&proc) {
            return Err("TF_GetThreadMgr 指针大小不一致".to_string());
        }
        // SAFETY: 非空的 GetProcAddress 结果，类型与 msctf!TF_GetThreadMgr 相同。
        let get_mgr: GetMgr = unsafe { std::mem::transmute_copy(&proc) };
        let mut raw = std::ptr::null_mut();
        // SAFETY: raw 是局部输出参数。成功时接口指针由 from_raw 接管并 Release。
        let hr = unsafe { get_mgr(&mut raw) };
        if hr.is_err() || raw.is_null() {
            if !raw.is_null() {
                // SAFETY: 失败时若仍给出接口指针，由 from_raw 接管，丢弃时 Release。
                drop(unsafe { ITfThreadMgr::from_raw(raw) });
            }
            return Err(format!("TF_GetThreadMgr 0x{:08X}", hr.0 as u32));
        }
        // SAFETY: TF_GetThreadMgr 成功返回的指针由调用方释放。不调用 Activate，以免动到已有的 TSF 焦点。
        Ok(unsafe { ITfThreadMgr::from_raw(raw) })
    }

    fn co_create_thread_mgr() -> Result<ITfThreadMgr, String> {
        // SAFETY: 取本线程的 TSF 线程管理器。不聚合，也不 Activate。
        let mgr: ITfThreadMgr =
            unsafe { CoCreateInstance(&CLSID_TF_ThreadMgr, None, CLSCTX_INPROC_SERVER) }
                .map_err(|err| hresult_text("CoCreateInstance", &err))?;
        Ok(mgr)
    }

    fn notify_layout(mgr: &ITfThreadMgr) -> Result<(), String> {
        // SAFETY: GetFocus 读取本线程当前的文档管理器，不改变焦点。
        let doc = unsafe { mgr.GetFocus() }.map_err(|err| hresult_text("GetFocus", &err))?;
        // SAFETY: GetTop 读取该文档最上面的上下文。
        let context = unsafe { doc.GetTop() }.map_err(|err| hresult_text("GetTop", &err))?;
        let services: ITfContextOwnerServices = context
            .cast()
            .map_err(|err| hresult_text("ITfContextOwnerServices", &err))?;
        // SAFETY: 通知文本服务布局已经变了，让它重新读取文字范围并打开候选窗。不改组合字符串。
        unsafe { services.OnLayoutChange() }.map_err(|err| hresult_text("OnLayoutChange", &err))
    }

    fn show_candidate_elements(mgr: &ITfThreadMgr) -> Result<u32, String> {
        let ui: ITfUIElementMgr = mgr
            .cast()
            .map_err(|err| hresult_text("ITfUIElementMgr", &err))?;
        // SAFETY: 枚举当前线程已经存在的 UI 元素。
        let enumerator: IEnumTfUIElements =
            unsafe { ui.EnumUIElements() }.map_err(|err| hresult_text("EnumUIElements", &err))?;
        let mut shown = 0u32;
        for _ in 0..8 {
            let mut element = [None];
            let mut fetched = 0u32;
            // SAFETY: element 是本函数的一格缓冲区。fetched 是局部变量。
            let next = unsafe { enumerator.Next(&mut element, &mut fetched) };
            if next.is_err() || fetched == 0 {
                break;
            }
            let Some(element) = element[0].take() else {
                break;
            };
            let Ok(candidate) = element.cast::<ITfCandidateListUIElement>() else {
                continue;
            };
            // SAFETY: 这是候选列表元素。Show(true) 让文本服务显示它，不改组合字符串。
            if unsafe { candidate.Show(true) }.is_ok() {
                shown = shown.saturating_add(1);
            }
        }
        Ok(shown)
    }

    fn hresult_text(label: &str, err: &windows::core::Error) -> String {
        format!("{label} 0x{:08X}", err.code().0 as u32)
    }

    fn correct_size(hwnd: HWND) {
        let Some(dpi) = dpi_of(hwnd) else {
            return;
        };
        let Some(current) = outer_of(hwnd) else {
            return;
        };
        let (width, height) = with_fix(|fix| fix.expected_size(dpi));
        if !size_mismatch(current.width(), current.height(), width, height) {
            return;
        }
        note(&format!(
            "DPI 外框 {current_w}x{current_h} 收到 dpi={dpi} 后的尺寸 {width}x{height}，按后者收",
            current_w = current.width(),
            current_h = current.height(),
        ));
        place(
            hwnd,
            OuterRect {
                left: current.left,
                top: current.top,
                right: current.left.saturating_add(width),
                bottom: current.top.saturating_add(height),
            },
        );
    }

    fn apply_geometry() {
        let Some(hwnd) = HWND_SLOT.with(|slot| slot.get()) else {
            return;
        };
        correct_size(hwnd);
        if let Some(dpi) = dpi_of(hwnd) {
            refresh_ime(hwnd, dpi);
        }
    }

    fn settle_on_loop() {
        apply_geometry();
        flush_notes();
    }

    fn queue_settle() {
        if SETTLE_QUEUED.with(|flag| flag.replace(true)) {
            return;
        }
        let _ = slint::invoke_from_event_loop(|| {
            SETTLE_QUEUED.with(|flag| flag.set(false));
            settle_on_loop();
        });
    }

    fn queue_settle_after_drag() {
        // 定时器要在事件循环里建。窗口过程正处于系统的拖动模态循环，这里只把回调排进去。
        let _ = slint::invoke_from_event_loop(|| {
            slint::Timer::single_shot(std::time::Duration::from_millis(50), settle_on_loop);
        });
    }

    fn handle_dpi(hwnd: HWND, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        let dpi = (wparam.0 & 0xFFFF) as u32;
        let suggested = suggested_rect(lparam);
        let dragging = with_fix(|fix| fix.dragging);
        let force = with_fix(|fix| {
            fix.begin_dpi(
                dpi,
                suggested.unwrap_or(OuterRect {
                    left: 0,
                    top: 0,
                    right: 0,
                    bottom: 0,
                }),
            )
        });
        // SAFETY: 先让 winit 更新自己的 scale_factor。建议矩形在 WINDOWPOSCHANGING 里换上。
        let result = unsafe { DefSubclassProc(hwnd, WM_DPICHANGED, wparam, lparam) };
        if force && let Some(want) = suggested {
            let before = outer_of(hwnd);
            if before.is_some_and(|current| current != want) {
                place(hwnd, want);
            }
            let after = outer_of(hwnd);
            note(&format!(
                "DPI 拖动 {dpi} 建议 {}x{} 放置前 {} 放置后 {}",
                want.width(),
                want.height(),
                fmt_size(before),
                fmt_size(after),
            ));
        }
        with_fix(|fix| fix.end_dpi());
        // 拖动还没结束时不改输入法。窗口还跨在两块屏幕上，这时重设会让微软拼音关掉候选窗。
        if refresh_ime_on_this_dpi_message(dragging) {
            apply_geometry();
            queue_settle();
            queue_settle_after_drag();
        }
        result
    }

    fn fmt_size(rect: Option<OuterRect>) -> String {
        match rect {
            Some(rect) => format!("{}x{}", rect.width(), rect.height()),
            None => "未知".to_string(),
        }
    }

    unsafe extern "system" fn subclass_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        match msg {
            WM_NCDESTROY => {
                // SAFETY: 卸掉本过程用同一个 id 装上的子类。
                unsafe {
                    let _ = RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID);
                }
                HWND_SLOT.with(|slot| slot.set(None));
                // SAFETY: 销毁消息交给下一个窗口过程。
                unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
            }
            WM_ENTERSIZEMOVE => {
                with_fix(|fix| fix.on_enter_size_move());
                // SAFETY: 拖动开始仍由 winit 处理。
                unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
            }
            WM_EXITSIZEMOVE => {
                // SAFETY: 先让 winit 结束拖动状态，再按记下的尺寸收外框。
                let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
                with_fix(|fix| fix.on_exit_size_move());
                apply_geometry();
                queue_settle();
                queue_settle_after_drag();
                result
            }
            WM_DPICHANGED => handle_dpi(hwnd, wparam, lparam),
            WM_WINDOWPOSCHANGING => {
                let dpi_now = dpi_of(hwnd).unwrap_or(0);
                rewrite_pos(lparam, dpi_now);
                // SAFETY: WINDOWPOS 已经改成要保住的外框，再交给 winit。
                unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
            }
            _ => {
                // SAFETY: 其余消息不改，交给下一个窗口过程。
                unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DragFix, LOGICAL_HEIGHT, LOGICAL_WIDTH, OuterRect, PosOverride, caret_at_dpi,
        round_physical, size_mismatch, winit_physical_after_scale,
    };

    #[test]
    fn logical_window_matches_slint() {
        assert_eq!(LOGICAL_WIDTH, 720.0);
        assert_eq!(LOGICAL_HEIGHT, 760.0);
        assert_eq!(round_physical(LOGICAL_WIDTH, 96), 720);
        assert_eq!(round_physical(LOGICAL_WIDTH, 144), 1080);
        assert_eq!(round_physical(LOGICAL_HEIGHT, 144), 1140);
    }

    #[test]
    fn high_to_low_bounce_grows_when_old_pixels_stick() {
        let start = round_physical(LOGICAL_WIDTH, 144) as u32;
        assert_eq!(start, 1080);
        // 第一次消息已经把缩放记成 96，第二次仍看见 1080 物理像素并回到 144。
        let grown = winit_physical_after_scale(start, 96, 144);
        assert_eq!(grown, 1620);
        assert!(grown > start);
    }

    #[test]
    fn caret_tracks_dpi() {
        assert_eq!(
            caret_at_dpi(100.0, 80.0, 1.0, 16.0, 96),
            super::CaretPx {
                x: 100,
                y: 80,
                width: 1,
                height: 16,
            }
        );
        assert_eq!(
            caret_at_dpi(100.0, 80.0, 1.0, 16.0, 144),
            super::CaretPx {
                x: 150,
                y: 120,
                width: 2,
                height: 24,
            }
        );
    }

    #[test]
    fn drag_locks_the_first_suggested_rect() {
        let mut fix = DragFix::default();
        fix.on_enter_size_move();
        let suggested = OuterRect {
            left: 10,
            top: 20,
            right: 730,
            bottom: 780,
        };
        let other = OuterRect {
            left: 0,
            top: 0,
            right: 2000,
            bottom: 2000,
        };
        assert!(fix.begin_dpi(96, suggested));
        assert_eq!(fix.pos_override(96), PosOverride::Full(suggested));
        assert!(!fix.begin_dpi(144, other));
        assert_eq!(fix.pos_override(144), PosOverride::Full(suggested));
        fix.end_dpi();
        assert!(fix.locked.is_some());
        fix.end_dpi();
        assert!(fix.locked.is_none());
        assert_eq!(fix.pos_override(96), PosOverride::Size(720, 760));
        fix.on_exit_size_move();
        assert_eq!(fix.pos_override(96), PosOverride::None);
        assert_eq!(fix.expected_size(96), (720, 760));
        assert_eq!(fix.expected_size(144), (1080, 1140));
    }

    #[test]
    fn one_pixel_is_not_a_mismatch() {
        assert!(!size_mismatch(720, 760, 721, 760));
        assert!(size_mismatch(1080, 1140, 720, 760));
    }

    #[test]
    fn drag_dpi_message_does_not_touch_ime() {
        assert!(!super::refresh_ime_on_this_dpi_message(true));
        assert!(super::refresh_ime_on_this_dpi_message(false));
    }

    #[test]
    fn ime_forms_follow_the_client_caret() {
        let forms = super::ime_forms(super::CaretPx {
            x: 59,
            y: 86,
            width: 1,
            height: 16,
        });
        assert_eq!(
            forms.composition_style,
            super::STYLE_POINT | super::STYLE_FORCE_POSITION
        );
        assert_eq!((forms.composition_x, forms.composition_y), (59, 102));
        assert_eq!(forms.exclude_style, super::STYLE_EXCLUDE);
        assert_eq!(
            (
                forms.area_left,
                forms.area_top,
                forms.area_right,
                forms.area_bottom
            ),
            (59, 86, 60, 102)
        );
        assert_eq!(forms.candidate_style, super::STYLE_CANDIDATEPOS);
        assert_eq!((forms.candidate_x, forms.candidate_y), (59, 102));
        assert_eq!(forms.area_right - forms.area_left, 1);
        assert_eq!(forms.area_bottom - forms.area_top, 16);
    }

    #[cfg(windows)]
    #[test]
    fn style_numbers_match_the_windows_crate() {
        use windows::Win32::UI::Input::Ime::{
            CFS_CANDIDATEPOS, CFS_EXCLUDE, CFS_FORCE_POSITION, CFS_POINT,
        };
        assert_eq!(super::STYLE_POINT, CFS_POINT);
        assert_eq!(super::STYLE_FORCE_POSITION, CFS_FORCE_POSITION);
        assert_eq!(super::STYLE_CANDIDATEPOS, CFS_CANDIDATEPOS);
        assert_eq!(super::STYLE_EXCLUDE, CFS_EXCLUDE);
    }
}
