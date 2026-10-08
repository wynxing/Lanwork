use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::CLASS_E_NOAGGREGATION;
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, CoRegisterClassObject, CoRevokeClassObject, IClassFactory,
    IClassFactory_Impl, REGCLS_MULTIPLEUSE,
};
use windows::Win32::UI::Notifications::{
    INotificationActivationCallback, INotificationActivationCallback_Impl,
    NOTIFICATION_USER_INPUT_DATA,
};
use windows::core::{GUID, IUnknown, Interface, PCWSTR, Ref, implement};

use crate::registry::activator_guid;
use crate::util::{SpikeResult, string_from_pcwstr, win_err};

static COOKIE: AtomicU32 = AtomicU32::new(0);

pub fn register_class() -> SpikeResult<()> {
    let factory: IClassFactory = Factory.into();
    let unknown: IUnknown = factory
        .cast()
        .map_err(|err| win_err("激活器转换为 IUnknown", err))?;
    let cookie = unsafe {
        CoRegisterClassObject(
            &activator_guid(),
            &unknown,
            CLSCTX_LOCAL_SERVER,
            REGCLS_MULTIPLEUSE,
        )
    }
    .map_err(|err| win_err("CoRegisterClassObject", err))?;
    COOKIE.store(cookie, Ordering::SeqCst);
    crate::util::log_line(&format!("已在进程内注册 COM 激活器，cookie={cookie}"));
    Ok(())
}

pub fn revoke_class() {
    let cookie = COOKIE.swap(0, Ordering::SeqCst);
    if cookie != 0 {
        unsafe {
            let _ = CoRevokeClassObject(cookie);
        }
        crate::util::log_line("已撤销进程内 COM 激活器");
    }
}

#[implement(INotificationActivationCallback)]
struct Activator;

impl INotificationActivationCallback_Impl for Activator_Impl {
    fn Activate(
        &self,
        _app_user_model_id: &PCWSTR,
        invoked_args: &PCWSTR,
        _data: *const NOTIFICATION_USER_INPUT_DATA,
        _count: u32,
    ) -> windows::core::Result<()> {
        let args = string_from_pcwstr(*invoked_args);
        crate::util::log_line(&format!("COM Activate launch={args}"));
        crate::panel::note_activation(&args);
        Ok(())
    }
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<'_, IUnknown>,
        iid: *const GUID,
        interface: *mut *mut c_void,
    ) -> windows::core::Result<()> {
        if !outer.is_null() {
            return Err(windows::core::Error::from(CLASS_E_NOAGGREGATION));
        }
        let callback: INotificationActivationCallback = Activator.into();
        unsafe { callback.query(iid, interface).ok() }
    }

    fn LockServer(&self, _lock: windows::core::BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}
