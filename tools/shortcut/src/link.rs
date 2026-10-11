//! 用 `IPropertyStore` 写开始菜单快捷方式。激活 CLSID 用 `VT_CLSID`。

use std::path::Path;

use windows::Win32::Foundation::{PROPERTYKEY, S_OK};
use windows::Win32::System::Com::StructuredStorage::{
    InitPropVariantFromCLSID, PROPVARIANT, PropVariantClear, PropVariantToString,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoTaskMemAlloc, CoUninitialize, IPersistFile, STGM_READ,
};
use windows::Win32::System::Variant::VARENUM;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{GUID, Interface, PCWSTR, PWSTR};

use crate::{
    ACTIVATOR_CLSID_U128, APP_USER_MODEL_FMTID, APP_USER_MODEL_ID_PID, APP_USER_MODEL_TOAST_PID,
    AUMID, DISPLAY_NAME,
};

const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(APP_USER_MODEL_FMTID),
    pid: APP_USER_MODEL_ID_PID,
};

const PKEY_APP_USER_MODEL_TOAST_ACTIVATOR: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(APP_USER_MODEL_FMTID),
    pid: APP_USER_MODEL_TOAST_PID,
};

struct ComGuard {
    uninit: bool,
}

impl ComGuard {
    fn new() -> Result<Self, String> {
        // SAFETY: 本进程这条线程上还没有别的套间。S_FALSE 表示已经初始化过，不能再 CoUninitialize。
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if hr.is_err() {
            return Err(format!("CoInitializeEx 失败: {hr:?}"));
        }
        Ok(Self { uninit: hr == S_OK })
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.uninit {
            // SAFETY: 与上面返回 S_OK 的 CoInitializeEx 成对，只释放这一次。
            unsafe { CoUninitialize() }
        }
    }
}

pub fn write(exe: &Path, lnk: &Path) -> Result<(), String> {
    let _com = ComGuard::new()?;
    let exe_text = exe
        .to_str()
        .filter(|text| !text.is_empty() && !text.contains('"'))
        .ok_or("程序路径为空、不是 Unicode，或含有引号")?;
    if let Some(parent) = lnk.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建快捷方式目录失败: {err}"))?;
    }
    // SAFETY: ShellLink 是本机的进程内类。下面的接口指针只在这次调用里使用。
    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
        .map_err(|err| format!("CoCreateInstance(ShellLink) 失败: {err}"))?;
    let exe_wide = wide(exe_text);
    // SAFETY: exe_wide 以 0 结尾，活过这次调用。
    unsafe { link.SetPath(pcwstr(&exe_wide)) }.map_err(|err| format!("SetPath 失败: {err}"))?;
    let empty = wide("");
    // SAFETY: empty 以 0 结尾，活过这次调用。
    unsafe { link.SetArguments(pcwstr(&empty)) }
        .map_err(|err| format!("SetArguments 失败: {err}"))?;
    let directory = exe
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or("程序路径没有目录")?;
    let directory_text = directory.to_str().ok_or("程序目录不是 Unicode")?;
    let directory_wide = wide(directory_text);
    // SAFETY: directory_wide 以 0 结尾，活过这次调用。
    unsafe { link.SetWorkingDirectory(pcwstr(&directory_wide)) }
        .map_err(|err| format!("SetWorkingDirectory 失败: {err}"))?;
    // SAFETY: 图标路径与 exe 相同。索引 0 是资源里的第一个图标，对应资源 ID 1。
    unsafe { link.SetIconLocation(pcwstr(&exe_wide), 0) }
        .map_err(|err| format!("SetIconLocation 失败: {err}"))?;
    let description = wide(DISPLAY_NAME);
    // SAFETY: description 以 0 结尾，活过这次调用。
    unsafe { link.SetDescription(pcwstr(&description)) }
        .map_err(|err| format!("SetDescription 失败: {err}"))?;
    // SAFETY: 只设置显示方式，不保留额外指针。
    unsafe { link.SetShowCmd(SW_SHOWNORMAL) }.map_err(|err| format!("SetShowCmd 失败: {err}"))?;

    let store: IPropertyStore = link
        .cast()
        .map_err(|err| format!("快捷方式 IPropertyStore 失败: {err}"))?;
    set_string_property(&store, &PKEY_APP_USER_MODEL_ID, AUMID)?;
    set_guid_property(&store, &PKEY_APP_USER_MODEL_TOAST_ACTIVATOR)?;
    // SAFETY: store 上的值已经写完。提交只刷新这块属性存储。
    unsafe { store.Commit() }.map_err(|err| format!("IPropertyStore::Commit 失败: {err}"))?;

    let file: IPersistFile = link
        .cast()
        .map_err(|err| format!("快捷方式 IPersistFile 失败: {err}"))?;
    let link_text = lnk.to_str().ok_or("快捷方式路径不是 Unicode")?;
    let link_wide = wide(link_text);
    // SAFETY: link_wide 以 0 结尾，活过 Save。true 让快捷方式记住这个路径。
    unsafe { file.Save(pcwstr(&link_wide), true) }
        .map_err(|err| format!("IPersistFile::Save 失败: {err}"))?;
    Ok(())
}

pub fn read(lnk: &Path) -> Result<(Option<String>, Option<String>), String> {
    let _com = ComGuard::new()?;
    // SAFETY: 与 write 相同，只在这次调用里使用返回的接口。
    let link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
        .map_err(|err| format!("读取快捷方式时 CoCreateInstance 失败: {err}"))?;
    let file: IPersistFile = link
        .cast()
        .map_err(|err| format!("读取快捷方式时 IPersistFile 失败: {err}"))?;
    let link_text = lnk.to_str().ok_or("快捷方式路径不是 Unicode")?;
    let link_wide = wide(link_text);
    // SAFETY: link_wide 以 0 结尾，活过 Load。STGM_READ 只读。
    unsafe { file.Load(pcwstr(&link_wide), STGM_READ) }
        .map_err(|err| format!("IPersistFile::Load 失败: {err}"))?;
    let store: IPropertyStore = link
        .cast()
        .map_err(|err| format!("读取快捷方式时 IPropertyStore 失败: {err}"))?;
    let aumid = read_string_property(&store, &PKEY_APP_USER_MODEL_ID)?;
    let activator = read_string_property(&store, &PKEY_APP_USER_MODEL_TOAST_ACTIVATOR)?;
    Ok((aumid, activator))
}

fn set_string_property(
    store: &IPropertyStore,
    key: &PROPERTYKEY,
    value: &str,
) -> Result<(), String> {
    let mut variant = string_propvariant(value)?;
    // SAFETY: key 指向本模块的静态 PROPERTYKEY。variant 是刚建的 VT_LPWSTR，指针在清除前有效。
    let result = unsafe { store.SetValue(key, &variant) };
    // SAFETY: variant 由 string_propvariant 创建，这里只释放一次。
    unsafe {
        let _ = PropVariantClear(&mut variant);
    }
    result.map_err(|err| format!("写入快捷方式字符串属性失败: {err}"))
}

fn set_guid_property(store: &IPropertyStore, key: &PROPERTYKEY) -> Result<(), String> {
    let guid = GUID::from_u128(ACTIVATOR_CLSID_U128);
    // SAFETY: guid 是局部变量，调用期间有效。返回的 PROPVARIANT 下面会清除。
    let mut variant = unsafe { InitPropVariantFromCLSID(&guid) }
        .map_err(|err| format!("InitPropVariantFromCLSID 失败: {err}"))?;
    // SAFETY: variant 的类型是 VT_CLSID，对应 ToastActivatorCLSID。key 是静态的。
    let result = unsafe { store.SetValue(key, &variant) };
    // SAFETY: variant 由 InitPropVariantFromCLSID 创建，这里只释放一次。
    unsafe {
        let _ = PropVariantClear(&mut variant);
    }
    result.map_err(|err| format!("写入快捷方式 CLSID 属性失败: {err}"))
}

fn read_string_property(
    store: &IPropertyStore,
    key: &PROPERTYKEY,
) -> Result<Option<String>, String> {
    // SAFETY: key 指向静态 PROPERTYKEY。返回的 variant 下面会清除。
    let mut variant = unsafe { store.GetValue(key) }
        .map_err(|err| format!("IPropertyStore::GetValue 失败: {err}"))?;
    let mut buf = [0u16; 300];
    // SAFETY: buf 可写，长度是 300 个 UTF-16 单元。variant 在转换期间有效。
    let converted = unsafe { PropVariantToString(&variant, &mut buf) };
    // SAFETY: variant 由 GetValue 创建，这里只释放一次。
    unsafe {
        let _ = PropVariantClear(&mut variant);
    }
    match converted {
        Ok(()) => {
            let text = string_from_wide(&buf);
            if text.is_empty() {
                Ok(None)
            } else {
                Ok(Some(text))
            }
        }
        Err(_) => Ok(None),
    }
}

fn string_propvariant(value: &str) -> Result<PROPVARIANT, String> {
    let units = wide(value);
    let bytes = units.len().checked_mul(2).ok_or("快捷方式字符串过长")?;
    // SAFETY: 分配 bytes 字节。失败时指针为空，不写。成功时这块内存交给 PROPVARIANT，由 PropVariantClear 释放。
    let memory = unsafe { CoTaskMemAlloc(bytes) } as *mut u16;
    if memory.is_null() {
        return Err("CoTaskMemAlloc 失败".to_string());
    }
    // SAFETY: memory 指向刚分配的 units.len() 个 u16，可写，不与 units 重叠。31 是 VT_LPWSTR。
    unsafe {
        std::ptr::copy_nonoverlapping(units.as_ptr(), memory, units.len());
        let mut variant = PROPVARIANT::default();
        let header = &mut *variant.Anonymous.Anonymous;
        header.vt = VARENUM(31);
        header.Anonymous.pwszVal = PWSTR(memory);
        Ok(variant)
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn pcwstr(buf: &[u16]) -> PCWSTR {
    PCWSTR::from_raw(buf.as_ptr())
}

fn string_from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|unit| *unit == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}
