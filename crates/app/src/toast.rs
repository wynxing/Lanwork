//! 到期通知的系统弹窗，以及安装版的点击激活。
//!
//! 用 WinRT `ToastNotificationManager`。不使用 Windows App SDK。
//! 安装版在平台线程注册激活类对象，便携版只补显示名，不注册点击。
//! 类对象放在 STA，不实现 `IAgileObject`。

use std::cell::{Cell, RefCell};
use std::sync::Mutex;

use lanwork_core::storage::Store;
use lanwork_core::todos::{DueReminder, toast_launch, toast_xml};
use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows::Win32::Foundation::{CLASS_E_NOAGGREGATION, E_POINTER};
use windows::Win32::System::Com::{
    CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoInitializeEx, CoRegisterClassObject,
    CoResumeClassObjects, CoRevokeClassObject, CoUninitialize, IClassFactory, IClassFactory_Impl,
    REGCLS_MULTIPLEUSE, REGCLS_SUSPENDED,
};
use windows::Win32::UI::Notifications::{
    INotificationActivationCallback, INotificationActivationCallback_Impl,
    NOTIFICATION_USER_INPUT_DATA,
};
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
use windows::core::{GUID, IUnknown, Interface, PCWSTR, Ref, implement};

use crate::winutil::{pcwstr, wide};

/// 与安装程序、`tools/shortcut` 里的激活 CLSID 相同。
const ACTIVATOR_CLSID: GUID = GUID::from_u128(0xE8E9_EF84_55BA_4A0C_B4DC_9F9F_4A28_068D);
const AUMID: &str = "Lanwork";

thread_local! {
    static STA_INIT: Cell<bool> = const { Cell::new(false) };
    static COOKIE: Cell<u32> = const { Cell::new(0) };
    static FACTORY: RefCell<Option<IUnknown>> = const { RefCell::new(None) };
}

fn pending() -> &'static Mutex<Vec<String>> {
    static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());
    &PENDING
}

fn lock(mutex: &Mutex<Vec<String>>) -> std::sync::MutexGuard<'_, Vec<String>> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 进程标识。便携版另外只写 `DisplayName`。
pub(crate) fn prepare(store: &Store, installed: bool) {
    let name = wide(AUMID);
    // SAFETY: 名字以 0 结尾，调用之后不再使用这块缓冲。
    if let Err(err) = unsafe { SetCurrentProcessExplicitAppUserModelID(pcwstr(&name)) } {
        store.log_warn(&format!("通知标识没有设上：{err}"));
    }
    if !installed && let Err(err) = crate::registry::write_portable_display_name() {
        store.log_warn(&format!("便携版通知显示名没有写上：{err}"));
    }
}

/// 工具通知。不带 `launch`，点击不会打开待办。
pub(crate) fn show_text(title: &str, body: &str) -> Result<(), String> {
    show_lines(title, std::slice::from_ref(&body.to_string()))
}

pub(crate) fn show_lines(title: &str, lines: &[String]) -> Result<(), String> {
    show_xml(&text_toast_xml(title, lines))
}

fn text_toast_xml(title: &str, lines: &[String]) -> String {
    let mut xml = String::from("<toast><visual><binding template=\"ToastGeneric\"><text>");
    xml.push_str(&xml_escape(title));
    xml.push_str("</text>");
    if lines.is_empty() {
        xml.push_str("<text></text>");
    }
    for line in lines {
        xml.push_str("<text>");
        xml.push_str(&xml_escape(line));
        xml.push_str("</text>");
    }
    xml.push_str("</binding></visual></toast>");
    xml
}

fn xml_escape(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(ch),
        }
    }
    out
}

pub(crate) fn show_one(item: &DueReminder) -> Result<(), String> {
    let xml = toast_xml(std::slice::from_ref(item)).ok_or("通知没有内容")?;
    show_xml(&xml)
}

pub(crate) fn show_missed(items: &[DueReminder]) -> Result<(), String> {
    let xml = toast_xml(items).ok_or("通知没有内容")?;
    show_xml(&xml)
}

fn show_xml(xml: &str) -> Result<(), String> {
    let document = XmlDocument::new().map_err(|err| err.to_string())?;
    document
        .LoadXml(&windows::core::HSTRING::from(xml))
        .map_err(|err| err.to_string())?;
    let toast =
        ToastNotification::CreateToastNotification(&document).map_err(|err| err.to_string())?;
    let notifier =
        ToastNotificationManager::CreateToastNotifierWithId(&windows::core::HSTRING::from(AUMID))
            .map_err(|err| err.to_string())?;
    notifier.Show(&toast).map_err(|err| err.to_string())
}

/// 平台线程调用。失败时调用方记日志，进程继续，只更新角标。
pub(crate) fn register_activator() -> Result<(), String> {
    // SAFETY: 平台线程此前没有初始化 COM。成功（含已经初始化）之后，退出时配对 CoUninitialize。
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if hr.is_err() {
        return Err(format!("通知激活的 COM 初始化失败: {hr}"));
    }
    STA_INIT.with(|cell| cell.set(true));
    let factory: IClassFactory = Factory.into();
    let unknown: IUnknown = factory.cast().map_err(|err| err.to_string())?;
    // SAFETY: 类对象活在本线程的 FACTORY 里，直到 revoke。REGCLS_SUSPENDED 让点击等到界面循环起来。
    let cookie = unsafe {
        CoRegisterClassObject(
            &ACTIVATOR_CLSID,
            &unknown,
            CLSCTX_LOCAL_SERVER,
            REGCLS_MULTIPLEUSE | REGCLS_SUSPENDED,
        )
    }
    .map_err(|err| format!("通知激活注册失败: {err}"))?;
    FACTORY.with(|slot| *slot.borrow_mut() = Some(unknown));
    COOKIE.with(|cell| cell.set(cookie));
    Ok(())
}

pub(crate) fn resume_activator() {
    if COOKIE.with(|cell| cell.get()) == 0 {
        return;
    }
    // SAFETY: 与 register_activator 同一线程。没注册时上面已经返回。
    if let Err(err) = unsafe { CoResumeClassObjects() } {
        note_runtime_error(format!("通知激活没有恢复：{err}"));
    }
}

pub(crate) fn take_runtime_error() -> Option<String> {
    runtime_errors()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

fn runtime_errors() -> &'static Mutex<Option<String>> {
    static ERRORS: Mutex<Option<String>> = Mutex::new(None);
    &ERRORS
}

fn note_runtime_error(message: String) {
    *runtime_errors()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(message);
}

pub(crate) fn revoke_activator() {
    let cookie = COOKIE.with(|cell| cell.replace(0));
    if cookie != 0 {
        // SAFETY: cookie 是本线程 CoRegisterClassObject 的返回值。
        unsafe {
            let _ = CoRevokeClassObject(cookie);
        }
    }
    FACTORY.with(|slot| slot.borrow_mut().take());
    if STA_INIT.with(|cell| cell.replace(false)) {
        // SAFETY: 与本线程成功的 CoInitializeEx 配对。
        unsafe { CoUninitialize() }
    }
}

pub(crate) fn flush_pending() {
    let ids: Vec<_> = lock(pending()).drain(..).collect();
    for id in ids {
        crate::panel::open_from_notification(&id);
    }
}

fn note_activation(args: &str) {
    if toast_launch(args).is_none() {
        return;
    }
    lock(pending()).push(args.trim().to_owned());
    let _ = slint::invoke_from_event_loop(flush_pending);
}

#[implement(IClassFactory)]
struct Factory;

#[allow(non_snake_case)]
impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<'_, IUnknown>,
        iid: *const GUID,
        object: *mut *mut std::ffi::c_void,
    ) -> windows::core::Result<()> {
        if outer.as_ref().is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        if object.is_null() || iid.is_null() {
            return Err(E_POINTER.into());
        }
        // SAFETY: COM 给出的输出指针在这次调用期间可写。失败时先清空。
        unsafe { *object = std::ptr::null_mut() };
        let callback: INotificationActivationCallback = Activator.into();
        let unknown: IUnknown = callback.cast()?;
        // SAFETY: iid 和 object 已排除空指针。query 按 iid 填写 object。
        unsafe { unknown.query(iid, object).ok() }
    }

    fn LockServer(&self, _lock: windows::core::BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}

#[implement(INotificationActivationCallback)]
struct Activator;

#[allow(non_snake_case)]
impl INotificationActivationCallback_Impl for Activator_Impl {
    fn Activate(
        &self,
        _app_user_model_id: &PCWSTR,
        invoked_args: &PCWSTR,
        _data: *const NOTIFICATION_USER_INPUT_DATA,
        _count: u32,
    ) -> windows::core::Result<()> {
        let args = pcwstr_string(invoked_args);
        note_activation(&args);
        Ok(())
    }
}

fn pcwstr_string(value: &PCWSTR) -> String {
    if value.is_null() {
        return String::new();
    }
    unsafe { value.to_string() }.unwrap_or_default()
}
