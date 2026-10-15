//! 设置页选择新的数据目录。只用系统的文件夹对话框，不读取文件内容。

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::PathBuf;

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemFree, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_PATHMUSTEXIST, FOS_PICKFOLDERS, FileOpenDialog, IFileOpenDialog,
    SIGDN_FILESYSPATH,
};
use windows::core::{HRESULT, HSTRING};

/// 用户选中的文件夹。取消返回 `Ok(None)`。打不开对话框时返回错误。
pub(crate) fn pick_folder(owner: HWND) -> Result<Option<PathBuf>, String> {
    // SAFETY: 对话框在界面线程上显示。本函数初始化的 COM 在返回前释放。
    unsafe { pick_folder_com(owner) }
}

unsafe fn pick_folder_com(owner: HWND) -> Result<Option<PathBuf>, String> {
    // SAFETY: 保留参数是空的。界面线程通常已经是 STA，这时返回 S_FALSE，引用计数仍要配对释放。
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let uninit = hr.is_ok();
    let result = unsafe { show_dialog(owner) };
    if uninit {
        // SAFETY: 与上面成功的 CoInitializeEx 配对，包括 S_FALSE。
        unsafe { CoUninitialize() };
    }
    result
}

unsafe fn show_dialog(owner: HWND) -> Result<Option<PathBuf>, String> {
    // SAFETY: FileOpenDialog 是系统里的进程内类。
    let dialog: IFileOpenDialog = unsafe {
        CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|_| "不能选择文件夹".to_owned())?
    };
    // SAFETY: dialog 是上面刚创建的接口。
    let options = unsafe { dialog.GetOptions() }.map_err(|_| "不能选择文件夹".to_owned())?;
    // SAFETY: 只加上文件夹、真实文件系统和路径必须存在。
    unsafe {
        dialog.SetOptions(options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST)
    }
    .map_err(|_| "不能选择文件夹".to_owned())?;
    // SAFETY: 标题是本进程的常量。
    unsafe { dialog.SetTitle(&HSTRING::from("选择数据目录")) }
        .map_err(|_| "不能选择文件夹".to_owned())?;
    // SAFETY: owner 是本进程的面板窗口。用户取消时是 HRESULT_FROM_WIN32(ERROR_CANCELLED)。
    match unsafe { dialog.Show(Some(owner)) } {
        Ok(()) => {}
        Err(err) if err.code() == HRESULT(0x8007_04C7_u32.cast_signed()) => return Ok(None),
        Err(_) => return Err("不能选择文件夹".to_owned()),
    }
    // SAFETY: 用户已经选中一项。
    let item = unsafe { dialog.GetResult() }.map_err(|_| "不能选择文件夹".to_owned())?;
    // SAFETY: 返回的字符串由下面的 CoTaskMemFree 释放。
    let name = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }
        .map_err(|_| "不能选择文件夹".to_owned())?;
    let path = path_from_pwstr(name);
    // SAFETY: name 由 GetDisplayName 用 CoTaskMemAlloc 分配，这里只释放一次，之后不再读。
    unsafe { CoTaskMemFree(Some(name.0.cast())) };
    if path.as_os_str().is_empty() {
        return Err("不能选择文件夹".to_owned());
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
