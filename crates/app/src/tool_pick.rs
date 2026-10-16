//! 选择要导入的工具包。文件夹和 zip 都从这一个对话框返回。
//!
//! 不要求选中的路径已经是文件。对话框的默认文件名是占位符，
//! 用户停在某个文件夹里确认时，返回的是「该文件夹 + 占位名」。
//! 占位名本身不出现在产品界面上。

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_NOCHANGEDIR, FOS_PATHMUSTEXIST, FileOpenDialog, IFileDialog,
    IFileOpenDialog, SIGDN_FILESYSPATH,
};
use windows::core::{HRESULT, HSTRING, PCWSTR};

use lanwork_toolhost::FOLDER_SENTINEL;

const CANNOT_PICK: &str = "不能选择文件";

/// 取消返回 `Ok(None)`。
pub(crate) fn pick_package(owner: HWND) -> Result<Option<PathBuf>, String> {
    // SAFETY: 对话框在界面线程上显示。本函数初始化的 COM 在返回前释放。
    unsafe { with_com(|| show(owner)) }
}

unsafe fn with_com(
    body: impl FnOnce() -> Result<Option<PathBuf>, String>,
) -> Result<Option<PathBuf>, String> {
    // SAFETY: 保留参数是空的。界面线程通常已经是 STA，这时返回 S_FALSE，引用计数仍要配对释放。
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let uninit = hr.is_ok();
    let result = body();
    if uninit {
        // SAFETY: 与上面成功的 CoInitializeEx 配对，包括 S_FALSE。
        unsafe { CoUninitialize() };
    }
    result
}

unsafe fn show(owner: HWND) -> Result<Option<PathBuf>, String> {
    // SAFETY: FileOpenDialog 是系统里的进程内类。
    let dialog: IFileOpenDialog = unsafe {
        CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).map_err(|_| CANNOT_PICK)?
    };
    let options = unsafe { dialog.GetOptions() }.map_err(|_| CANNOT_PICK.to_owned())?;
    // 不设 FOS_FILEMUSTEXIST。文件夹确认时返回的是占位文件名，那个文件并不存在。
    unsafe {
        dialog.SetOptions(options | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_NOCHANGEDIR)
    }
    .map_err(|_| CANNOT_PICK.to_owned())?;
    let filter = unsafe { prepare(&dialog) }?;
    let suggested = HSTRING::from(FOLDER_SENTINEL);
    unsafe { dialog.SetFileName(&suggested) }.map_err(|_| CANNOT_PICK.to_owned())?;
    let picked = finish(&dialog, owner);
    drop(filter);
    picked
}

struct ZipFilter {
    name: HSTRING,
    spec: HSTRING,
    ext: HSTRING,
    title: HSTRING,
}

unsafe fn prepare(dialog: &IFileDialog) -> Result<ZipFilter, String> {
    let filter = ZipFilter {
        name: HSTRING::from("Zip"),
        spec: HSTRING::from("*.zip"),
        ext: HSTRING::from("zip"),
        title: HSTRING::from("选择工具包"),
    };
    let types = [COMDLG_FILTERSPEC {
        pszName: PCWSTR(filter.name.as_ptr()),
        pszSpec: PCWSTR(filter.spec.as_ptr()),
    }];
    // SAFETY: 筛选字符串由返回的 ZipFilter 持有，调用方要留到 Show 返回。
    unsafe { dialog.SetFileTypes(&types) }.map_err(|_| CANNOT_PICK.to_owned())?;
    unsafe { dialog.SetFileTypeIndex(1) }.map_err(|_| CANNOT_PICK.to_owned())?;
    unsafe { dialog.SetDefaultExtension(&filter.ext) }.map_err(|_| CANNOT_PICK.to_owned())?;
    unsafe { dialog.SetTitle(&filter.title) }.map_err(|_| CANNOT_PICK.to_owned())?;
    Ok(filter)
}

fn finish(dialog: &IFileDialog, owner: HWND) -> Result<Option<PathBuf>, String> {
    // SAFETY: owner 是本进程的面板窗口。用户取消时是 HRESULT_FROM_WIN32(ERROR_CANCELLED)。
    match unsafe { dialog.Show(Some(owner)) } {
        Ok(()) => {}
        Err(err) if err.code() == HRESULT(0x8007_04C7_u32.cast_signed()) => return Ok(None),
        Err(_) => return Err(CANNOT_PICK.to_owned()),
    }
    // SAFETY: 用户已经选中一项。
    let item = unsafe { dialog.GetResult() }.map_err(|_| CANNOT_PICK.to_owned())?;
    // SAFETY: 返回的字符串由下面的 CoTaskMemFree 释放。
    let name =
        unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }.map_err(|_| CANNOT_PICK.to_owned())?;
    let path = path_from_pwstr(name);
    // SAFETY: name 由 GetDisplayName 用 CoTaskMemAlloc 分配，这里只释放一次，之后不再读。
    unsafe { CoTaskMemFree(Some(name.0.cast())) };
    if path.as_os_str().is_empty() {
        return Err(CANNOT_PICK.to_owned());
    }
    Ok(Some(path))
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
