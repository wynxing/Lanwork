use std::path::Path;

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, PROPERTYKEY};
use windows::Win32::System::Com::StructuredStorage::{
    InitPropVariantFromCLSID, PROPVARIANT, PropVariantClear, PropVariantToString,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemAlloc, IPersistFile, STGM_READ,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows::Win32::System::Variant::VARENUM;
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
use windows::core::{GUID, Interface, PCWSTR, PWSTR};

use crate::model::{
    ACTIVATOR_CLSID, ACTIVATOR_CLSID_U128, AUMID, AUMID_KEY, CLSID_KEY, DISPLAY_NAME,
    LOCAL_SERVER_KEY, RegistrationFacts, RegistrationKind, classify, format_facts,
};
use crate::util::{
    SpikeError, SpikeResult, pcwstr, string_from_wide, wide, win_err, win32_missing_ok,
    win32_to_result,
};

pub enum Mode {
    Installed,
    Portable,
}

struct RegKey(HKEY);

impl Drop for RegKey {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }
}

pub fn activator_guid() -> GUID {
    GUID::from_u128(ACTIVATOR_CLSID_U128)
}

pub fn register(mode: Mode) -> SpikeResult<()> {
    remove_registration_artifacts()?;
    let exe = crate::util::exe_path()?;
    match mode {
        Mode::Installed => write_installed(&exe)?,
        Mode::Portable => write_portable()?,
    }
    Ok(())
}

pub fn unregister() -> SpikeResult<()> {
    remove_registration_artifacts()?;
    match crate::notify::clear_history() {
        Ok(()) => {}
        Err(err) => {
            crate::util::log_line(&format!("清除通知历史失败（注册项已删除）: {err}"));
        }
    }
    Ok(())
}

pub fn remove_registration_artifacts() -> SpikeResult<()> {
    delete_shortcut()?;
    delete_tree(LOCAL_SERVER_KEY)?;
    delete_tree(CLSID_KEY)?;
    delete_tree(AUMID_KEY)?;
    Ok(())
}

pub fn inspect() -> SpikeResult<RegistrationFacts> {
    let exe = crate::util::exe_path()?;
    let shortcut = crate::util::shortcut_path()?;
    let shortcut_present = shortcut.is_file();
    let (shortcut_aumid, shortcut_activator) = if shortcut_present {
        read_shortcut(&shortcut)?
    } else {
        (None, None)
    };
    Ok(RegistrationFacts {
        has_aumid_key: key_exists(AUMID_KEY)?,
        display_name: read_sz(AUMID_KEY, Some("DisplayName"))?,
        custom_activator: read_sz(AUMID_KEY, Some("CustomActivator"))?,
        local_server: read_sz(LOCAL_SERVER_KEY, None)?,
        shortcut_present,
        shortcut_aumid,
        shortcut_activator,
        exe_path: exe.display().to_string(),
    })
}

pub fn print_status() -> SpikeResult<()> {
    let facts = inspect()?;
    println!("{}", format_facts(&facts));
    println!("当前程序：{}", facts.exe_path);
    println!("快捷方式：{}", crate::util::shortcut_path()?.display());
    println!("判定枚举：{}", kind_name(classify(&facts)));
    Ok(())
}

fn kind_name(kind: RegistrationKind) -> &'static str {
    crate::model::kind_label(kind)
}

fn write_portable() -> SpikeResult<()> {
    let key = create_key(AUMID_KEY)?;
    set_sz(&key, Some("DisplayName"), DISPLAY_NAME)?;
    Ok(())
}

fn write_installed(exe: &Path) -> SpikeResult<()> {
    let aumid = create_key(AUMID_KEY)?;
    set_sz(&aumid, Some("DisplayName"), DISPLAY_NAME)?;
    set_sz(&aumid, Some("CustomActivator"), ACTIVATOR_CLSID)?;
    let clsid = create_key(CLSID_KEY)?;
    set_sz(&clsid, None, DISPLAY_NAME)?;
    let server = create_key(LOCAL_SERVER_KEY)?;
    set_sz(&server, None, &quoted_command(exe)?)?;
    write_shortcut(&crate::util::shortcut_path()?, exe)?;
    Ok(())
}

fn quoted_command(exe: &Path) -> SpikeResult<String> {
    let text = exe.display().to_string();
    if text.contains('"') {
        return Err(SpikeError::new("程序路径含有引号，不能写入 LocalServer32"));
    }
    Ok(format!("\"{text}\""))
}

fn create_key(subkey: &str) -> SpikeResult<RegKey> {
    let name = wide(subkey);
    let mut handle = HKEY::default();
    let error = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            pcwstr(&name),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            None,
            &mut handle,
            None,
        )
    };
    win32_to_result("RegCreateKeyExW", error)?;
    Ok(RegKey(handle))
}

fn set_sz(key: &RegKey, value_name: Option<&str>, value: &str) -> SpikeResult<()> {
    let name_buf;
    let name = match value_name {
        Some(text) => {
            name_buf = wide(text);
            pcwstr(&name_buf)
        }
        None => PCWSTR::null(),
    };
    let data = wide(value);
    let bytes: Vec<u8> = data.iter().flat_map(|unit| unit.to_le_bytes()).collect();
    let error = unsafe { RegSetValueExW(key.0, name, Some(0), REG_SZ, Some(bytes.as_slice())) };
    win32_to_result("RegSetValueExW", error)
}

fn delete_tree(subkey: &str) -> SpikeResult<()> {
    let name = wide(subkey);
    let error = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, pcwstr(&name)) };
    if error == ERROR_SUCCESS || error == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(SpikeError::new(format!(
            "删除注册表项失败 {subkey}: Win32 {}",
            error.0
        )))
    }
}

fn key_exists(subkey: &str) -> SpikeResult<bool> {
    let name = wide(subkey);
    let mut handle = HKEY::default();
    let error = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            pcwstr(&name),
            Some(0),
            KEY_READ,
            &mut handle,
        )
    };
    if error == ERROR_SUCCESS {
        unsafe {
            let _ = RegCloseKey(handle);
        }
        Ok(true)
    } else {
        win32_missing_ok(error)
    }
}

fn read_sz(subkey: &str, value_name: Option<&str>) -> SpikeResult<Option<String>> {
    let sub = wide(subkey);
    let name_buf;
    let name = match value_name {
        Some(text) => {
            name_buf = wide(text);
            pcwstr(&name_buf)
        }
        None => PCWSTR::null(),
    };
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let error = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            pcwstr(&sub),
            name,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if !win32_missing_ok(error)? {
        return Ok(None);
    }
    Ok(Some(string_from_wide(&buf)))
}

fn delete_shortcut() -> SpikeResult<()> {
    let path = crate::util::shortcut_path()?;
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(SpikeError::new(format!(
            "删除快捷方式失败 {}: {err}",
            path.display()
        ))),
    }
}

fn write_shortcut(path: &Path, exe: &Path) -> SpikeResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| SpikeError::new(format!("创建开始菜单目录失败: {err}")))?;
    }
    unsafe { write_shortcut_unchecked(path, exe) }
}

unsafe fn write_shortcut_unchecked(path: &Path, exe: &Path) -> SpikeResult<()> {
    let link: IShellLinkW = unsafe {
        CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|err| win_err("CoCreateInstance(ShellLink)", err))?
    };
    let exe_wide = wide(&exe.display().to_string());
    unsafe { link.SetPath(pcwstr(&exe_wide)) }
        .map_err(|err| win_err("IShellLink::SetPath", err))?;
    let arguments = wide("");
    unsafe { link.SetArguments(pcwstr(&arguments)) }
        .map_err(|err| win_err("IShellLink::SetArguments", err))?;
    let directory = exe.parent().unwrap_or(exe);
    let directory_wide = wide(&directory.display().to_string());
    unsafe { link.SetWorkingDirectory(pcwstr(&directory_wide)) }
        .map_err(|err| win_err("IShellLink::SetWorkingDirectory", err))?;
    let description = wide("Lanwork 通知验证，用 unregister 删除");
    unsafe { link.SetDescription(pcwstr(&description)) }
        .map_err(|err| win_err("IShellLink::SetDescription", err))?;

    let store: IPropertyStore = link
        .cast()
        .map_err(|err| win_err("快捷方式 IPropertyStore", err))?;
    set_string_property(&store, &PKEY_APP_USER_MODEL_ID, AUMID)?;
    set_guid_property(
        &store,
        &PKEY_APP_USER_MODEL_TOAST_ACTIVATOR,
        activator_guid(),
    )?;
    unsafe { store.Commit() }.map_err(|err| win_err("IPropertyStore::Commit", err))?;

    let file: IPersistFile = link
        .cast()
        .map_err(|err| win_err("快捷方式 IPersistFile", err))?;
    let link_wide = wide(&path.display().to_string());
    let link_pcw = pcwstr(&link_wide);
    unsafe { file.Save(link_pcw, true) }.map_err(|err| win_err("IPersistFile::Save", err))?;
    Ok(())
}

fn read_shortcut(path: &Path) -> SpikeResult<(Option<String>, Option<String>)> {
    unsafe { read_shortcut_unchecked(path) }
}

unsafe fn read_shortcut_unchecked(path: &Path) -> SpikeResult<(Option<String>, Option<String>)> {
    let link: IShellLinkW = unsafe {
        CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|err| win_err("读取快捷方式时 CoCreateInstance", err))?
    };
    let file: IPersistFile = link
        .cast()
        .map_err(|err| win_err("读取快捷方式时 IPersistFile", err))?;
    let link_wide = wide(&path.display().to_string());
    let link_pcw = pcwstr(&link_wide);
    unsafe { file.Load(link_pcw, STGM_READ) }.map_err(|err| win_err("IPersistFile::Load", err))?;
    let store: IPropertyStore = link
        .cast()
        .map_err(|err| win_err("读取快捷方式时 IPropertyStore", err))?;
    let aumid = read_string_property(&store, &PKEY_APP_USER_MODEL_ID)?;
    let activator = read_string_property(&store, &PKEY_APP_USER_MODEL_TOAST_ACTIVATOR)?;
    Ok((aumid, activator))
}

fn set_string_property(store: &IPropertyStore, key: &PROPERTYKEY, value: &str) -> SpikeResult<()> {
    let mut variant = string_propvariant(value)?;
    let result = unsafe { store.SetValue(key, &variant) };
    unsafe {
        let _ = PropVariantClear(&mut variant);
    }
    result.map_err(|err| win_err("写入快捷方式字符串属性", err))
}

fn set_guid_property(store: &IPropertyStore, key: &PROPERTYKEY, guid: GUID) -> SpikeResult<()> {
    let mut variant = unsafe { InitPropVariantFromCLSID(&guid) }
        .map_err(|err| win_err("InitPropVariantFromCLSID", err))?;
    let result = unsafe { store.SetValue(key, &variant) };
    unsafe {
        let _ = PropVariantClear(&mut variant);
    }
    result.map_err(|err| win_err("写入快捷方式 CLSID 属性", err))
}

fn read_string_property(store: &IPropertyStore, key: &PROPERTYKEY) -> SpikeResult<Option<String>> {
    let mut variant =
        unsafe { store.GetValue(key) }.map_err(|err| win_err("IPropertyStore::GetValue", err))?;
    let mut buf = [0u16; 300];
    let converted = unsafe { PropVariantToString(&variant, &mut buf) };
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

fn string_propvariant(value: &str) -> SpikeResult<PROPVARIANT> {
    let units = wide(value);
    let memory = unsafe { CoTaskMemAlloc(units.len() * 2) } as *mut u16;
    if memory.is_null() {
        return Err(SpikeError::new("CoTaskMemAlloc 失败"));
    }
    unsafe {
        std::ptr::copy_nonoverlapping(units.as_ptr(), memory, units.len());
        let mut variant = PROPVARIANT::default();
        let header = &mut *variant.Anonymous.Anonymous;
        header.vt = VARENUM(31);
        header.Anonymous.pwszVal = PWSTR(memory);
        Ok(variant)
    }
}

const PKEY_APP_USER_MODEL_ID: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9F4C_2855_9F79_4B39_A8D0_E1D4_2DE1_D5F3),
    pid: 5,
};

const PKEY_APP_USER_MODEL_TOAST_ACTIVATOR: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9F4C_2855_9F79_4B39_A8D0_E1D4_2DE1_D5F3),
    pid: 26,
};
