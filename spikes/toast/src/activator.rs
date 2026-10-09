use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::CLASS_E_NOAGGREGATION;
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, CoRegisterClassObject, CoResumeClassObjects, CoRevokeClassObject,
    IClassFactory, IClassFactory_Impl, REGCLS_MULTIPLEUSE, REGCLS_SUSPENDED,
};
use windows::Win32::UI::Notifications::{
    INotificationActivationCallback, INotificationActivationCallback_Impl,
    NOTIFICATION_USER_INPUT_DATA,
};
use windows::core::{GUID, IUnknown, Interface, PCWSTR, Ref, implement};

use crate::model::{ACTIVATOR_CLSID_U128, AUMID};
use crate::registry::activator_guid;
use crate::util::{SpikeResult, pcwstr, string_from_pcwstr, wide, win_err};

/// 只在内存里注册，不写注册表。本机已有 toast.exe 占着产品 CLSID 时，自检用它。
pub const PROBE_CLSID_U128: u128 = 0xB7E3_A1C2_4D55_4E18_9A60_2F6C_8D0E_11A7;

static COOKIE: AtomicU32 = AtomicU32::new(0);

pub fn probe_guid() -> GUID {
    GUID::from_u128(PROBE_CLSID_U128)
}

pub fn hex_clsid(value: u128) -> String {
    format!("{value:032X}")
}

pub fn guid_from_hex(text: &str) -> SpikeResult<GUID> {
    let hex: String = text.chars().filter(|ch| ch.is_ascii_hexdigit()).collect();
    if hex.len() != 32 {
        return Err(crate::util::SpikeError::new(format!(
            "CLSID 不是 32 位十六进制：{text}"
        )));
    }
    let value = u128::from_str_radix(&hex, 16)
        .map_err(|_| crate::util::SpikeError::new(format!("CLSID 无法解析：{text}")))?;
    Ok(GUID::from_u128(value))
}

pub fn register_class() -> SpikeResult<()> {
    register_class_guid(activator_guid(), ACTIVATOR_CLSID_U128)
}

pub fn register_class_guid(guid: GUID, value: u128) -> SpikeResult<()> {
    let flags = REGCLS_MULTIPLEUSE | REGCLS_SUSPENDED;
    crate::util::log_line(&format!(
        "CoRegisterClassObject clsid={} CLSCTX_LOCAL_SERVER REGCLS_MULTIPLEUSE|REGCLS_SUSPENDED flags=0x{:X} Agile=false",
        hex_clsid(value),
        flags.0
    ));
    let factory: IClassFactory = Factory.into();
    let unknown: IUnknown = factory
        .cast()
        .map_err(|err| win_err("激活器转换为 IUnknown", err))?;
    let cookie = unsafe { CoRegisterClassObject(&guid, &unknown, CLSCTX_LOCAL_SERVER, flags) }
        .map_err(|err| win_err("CoRegisterClassObject", err))?;
    // COM 自己持有一份引用。再留一份，避免工厂在恢复类对象之前被释放。
    std::mem::forget(unknown);
    COOKIE.store(cookie, Ordering::SeqCst);
    crate::util::log_line(&format!(
        "已在进程内注册 COM 激活器，cookie={cookie}，尚未 CoResumeClassObjects"
    ));
    Ok(())
}

pub fn resume_class() -> SpikeResult<()> {
    unsafe { CoResumeClassObjects() }.map_err(|err| win_err("CoResumeClassObjects", err))?;
    crate::util::log_line("CoResumeClassObjects 完成，STA 消息循环可以派发激活");
    Ok(())
}

pub fn revoke_class() {
    let cookie = COOKIE.swap(0, Ordering::SeqCst);
    if cookie != 0 {
        unsafe {
            let _ = CoRevokeClassObject(cookie);
        }
        crate::util::log_line(&format!("已撤销进程内 COM 激活器 cookie={cookie}"));
    }
}

pub fn activate_in_process(launch: &str) -> SpikeResult<()> {
    crate::util::log_line(&format!(
        "进程内 IClassFactory::CreateInstance + Activate launch={launch}"
    ));
    let factory: IClassFactory = Factory.into();
    let callback: INotificationActivationCallback = unsafe { factory.CreateInstance(None) }
        .map_err(|err| win_err("进程内 CreateInstance", err))?;
    call_activate(&callback, launch)
}

pub fn call_activate(callback: &INotificationActivationCallback, launch: &str) -> SpikeResult<()> {
    let aumid = wide(AUMID);
    let args = wide(launch);
    unsafe { callback.Activate(pcwstr(&aumid), pcwstr(&args), &[]) }
        .map_err(|err| win_err("INotificationActivationCallback::Activate", err))?;
    crate::util::log_line(&format!("Activate 调用返回 launch={launch}"));
    Ok(())
}

#[implement(INotificationActivationCallback, Agile = false)]
struct Activator;

impl INotificationActivationCallback_Impl for Activator_Impl {
    fn Activate(
        &self,
        app_user_model_id: &PCWSTR,
        invoked_args: &PCWSTR,
        _data: *const NOTIFICATION_USER_INPUT_DATA,
        _count: u32,
    ) -> windows::core::Result<()> {
        let args = string_from_pcwstr(*invoked_args);
        let aumid = string_from_pcwstr(*app_user_model_id);
        crate::util::log_line(&format!("COM Activate launch={args}"));
        crate::util::log_line(&format!("COM Activate aumid={aumid}"));
        crate::panel::note_activation(&args);
        Ok(())
    }
}

#[implement(IClassFactory, Agile = false)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<'_, IUnknown>,
        iid: *const GUID,
        interface: *mut *mut c_void,
    ) -> windows::core::Result<()> {
        let iid_text = if iid.is_null() {
            "null".to_string()
        } else {
            format!("{:?}", unsafe { *iid })
        };
        crate::util::log_line(&format!("COM CreateInstance iid={iid_text}"));
        if !outer.is_null() {
            crate::util::log_line("COM CreateInstance 拒绝聚合 CLASS_E_NOAGGREGATION");
            return Err(windows::core::Error::from(CLASS_E_NOAGGREGATION));
        }
        let callback: INotificationActivationCallback = Activator.into();
        let hr = unsafe { callback.query(iid, interface) };
        crate::util::log_line(&format!(
            "COM CreateInstance QueryInterface hr=0x{:08X}",
            hr.0 as u32
        ));
        hr.ok()
    }

    fn LockServer(&self, lock: windows::core::BOOL) -> windows::core::Result<()> {
        crate::util::log_line(&format!("COM LockServer lock={}", lock.as_bool()));
        Ok(())
    }
}
