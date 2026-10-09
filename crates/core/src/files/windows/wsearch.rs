//! Windows Search 文件名查询。
//!
//! 进程内 ADO，提供程序 `Search.CollatorDSO.1`，SQL 只含 `System.FileName LIKE`。
//! 连接字符串优先用 `ISearchManager` 给出的那条，失败再用字面量。
//! 不使用 `ISearchQueryHelper` 生成的 `CONTAINS(*)`。

use std::mem::{self, ManuallyDrop};

use windows::Win32::System::Com::{
    CLSCTX_ALL, CLSCTX_INPROC_SERVER, CLSIDFromProgID, CoCreateInstance, CoTaskMemFree,
    DISPATCH_FLAGS, DISPATCH_METHOD, DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT, DISPPARAMS,
    EXCEPINFO, IDispatch,
};
use windows::Win32::System::Search::{CSearchManager, ISearchManager};
use windows::Win32::System::Services::{
    CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatus, SC_MANAGER_CONNECT,
    SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_STATUS,
};
use windows::Win32::System::Variant::{
    VARIANT, VT_BOOL, VT_BSTR, VT_DISPATCH, VT_EMPTY, VT_ERROR, VT_I4, VT_NULL,
};
use windows::core::{BSTR, GUID, w};

use super::com::ensure_com;
use super::wide::{pcwstr, wide_null};
use crate::files::backend::{ProbeNote, ProbeNoteKind, SourceFailure, WindowsSearchProbe};
use crate::files::model::{
    FileHit, FileKind, MAX_FILE_RESULTS, WindowsSearchStatus, kind_from_item_type,
};
use crate::files::service::windows_search_down_line;
use crate::files::sql::{filename_like_sql, sql_is_filename_only};

const DISPID_PROPERTYPUT: i32 = -3;
const DISP_E_PARAMNOTFOUND: i32 = 0x8002_0004_u32 as i32;
const AD_CMD_TEXT: i32 = 1;
const LITERAL_CONNECTION: &str =
    "Provider=Search.CollatorDSO.1;Extended Properties=\"Application=Windows\"";

#[derive(Debug)]
pub(crate) struct Wsearch;

impl Wsearch {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn probe(&self) -> WindowsSearchProbe {
        match service_state() {
            ServiceState::Running => WindowsSearchProbe {
                status: WindowsSearchStatus::Available,
                notes: Vec::new(),
            },
            ServiceState::NotRunning => WindowsSearchProbe {
                status: WindowsSearchStatus::Unavailable,
                notes: vec![down("WSearch 服务未运行")],
            },
            ServiceState::Unknown => WindowsSearchProbe {
                status: WindowsSearchStatus::Available,
                notes: Vec::new(),
            },
        }
    }

    pub(crate) fn query(&self, text: &str) -> Result<Vec<FileHit>, SourceFailure> {
        let sql = filename_like_sql(text);
        if !sql_is_filename_only(&sql) {
            return Err(SourceFailure::new("查询语句不是只按文件名"));
        }
        ensure_com().map_err(SourceFailure::new)?;
        let session = Session::open()?;
        session.execute(&sql)
    }
}

enum ServiceState {
    Running,
    NotRunning,
    Unknown,
}

fn service_state() -> ServiceState {
    // SAFETY: 只查询 WSearch 的状态，不启动也不停止。
    let scm = match unsafe { OpenSCManagerW(None, None, SC_MANAGER_CONNECT) } {
        Ok(scm) => scm,
        Err(_) => return ServiceState::Unknown,
    };
    // SAFETY: scm 仍打开。只查询 WSearch，不启动也不停止。
    let opened = unsafe { OpenServiceW(scm, w!("WSearch"), SERVICE_QUERY_STATUS) };
    let service = match opened {
        Ok(service) => service,
        Err(err) => {
            // SAFETY: scm 是刚打开的句柄，这里只关闭它。
            unsafe {
                let _ = CloseServiceHandle(scm);
            }
            return if err.code().0 as u32 == 0x8007_0424 {
                ServiceState::NotRunning
            } else {
                ServiceState::Unknown
            };
        }
    };
    let mut status = SERVICE_STATUS::default();
    // SAFETY: service 已打开，status 是本函数的局部变量。
    let queried = unsafe { QueryServiceStatus(service, &mut status) };
    // SAFETY: 两个句柄都由本函数打开，查询结束后关闭。
    unsafe {
        let _ = CloseServiceHandle(service);
        let _ = CloseServiceHandle(scm);
    }
    match queried {
        Ok(()) if status.dwCurrentState == SERVICE_RUNNING => ServiceState::Running,
        Ok(()) => ServiceState::NotRunning,
        Err(_) => ServiceState::Unknown,
    }
}

fn down(detail: &str) -> ProbeNote {
    ProbeNote {
        kind: ProbeNoteKind::WindowsSearchDown,
        line: windows_search_down_line(detail),
    }
}

struct Session {
    conn: IDispatch,
}

impl Session {
    fn open() -> Result<Self, SourceFailure> {
        let connection_string =
            helper_connection_string().unwrap_or_else(|_| LITERAL_CONNECTION.to_owned());
        // SAFETY: 程序 ID 是字面量 ADODB.Connection，调用期间有效。
        let clsid = unsafe { CLSIDFromProgID(w!("ADODB.Connection")) }
            .map_err(|err| SourceFailure::new(hresult_detail("ADODB.Connection", &err)))?;
        // SAFETY: CLSID 来自上一步。只要进程内的 ADO 连接，不创建窗口。
        let conn: IDispatch = unsafe { CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER) }
            .map_err(|err| SourceFailure::new(hresult_detail("ADODB.Connection", &err)))?;
        let mut timeout = variant_i4(20);
        put_property(&conn, "CommandTimeout", &mut timeout.0)?;
        let mut args = [
            variant_missing(),
            variant_missing(),
            variant_missing(),
            variant_bstr(&connection_string),
        ];
        call(&conn, "Open", DISPATCH_METHOD, &mut args)?;
        Ok(Self { conn })
    }

    fn execute(&self, sql: &str) -> Result<Vec<FileHit>, SourceFailure> {
        let mut args = [
            variant_i4(AD_CMD_TEXT),
            variant_missing(),
            variant_bstr(sql),
        ];
        let result = call(&self.conn, "Execute", DISPATCH_METHOD, &mut args)?;
        let recordset = variant_dispatch(&result.0)?;
        let rows = read_rows(&recordset);
        let _ = call(&recordset, "Close", DISPATCH_METHOD, &mut []);
        rows
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = call(&self.conn, "Close", DISPATCH_METHOD, &mut []);
    }
}

fn read_rows(recordset: &IDispatch) -> Result<Vec<FileHit>, SourceFailure> {
    let mut hits = Vec::new();
    loop {
        if eof(recordset)? || hits.len() >= MAX_FILE_RESULTS {
            break;
        }
        let path = field_string(recordset, 0)?;
        let name = field_string(recordset, 1)?;
        let item_type = field_string(recordset, 3)?;
        if let Some(hit) = hit_from_row(name, path, &item_type) {
            hits.push(hit);
        }
        call(recordset, "MoveNext", DISPATCH_METHOD, &mut [])?;
    }
    Ok(hits)
}

fn hit_from_row(name: String, path: String, item_type: &str) -> Option<FileHit> {
    let name = if name.is_empty() {
        path.rsplit(['\\', '/']).next().unwrap_or("").to_owned()
    } else {
        name
    };
    if name.is_empty() && path.is_empty() {
        return None;
    }
    let kind = match kind_from_item_type(item_type) {
        crate::files::model::FileKind::Folder => FileKind::Folder,
        crate::files::model::FileKind::File => FileKind::File,
    };
    Some(FileHit { name, path, kind })
}

fn helper_connection_string() -> Result<String, SourceFailure> {
    ensure_com().map_err(SourceFailure::new)?;
    // SAFETY: SearchIndexer 注册为 LocalServer32。只请求进程内会得到 0x80040154，所以用 CLSCTX_ALL。这里只读连接字符串，不用它生成 SQL。
    let text = unsafe {
        let manager: ISearchManager = CoCreateInstance(&CSearchManager, None, CLSCTX_ALL)
            .map_err(|err| SourceFailure::new(hresult_detail("CSearchManager", &err)))?;
        let catalog = manager
            .GetCatalog(w!("SystemIndex"))
            .map_err(|err| SourceFailure::new(hresult_detail("SystemIndex", &err)))?;
        let helper = catalog
            .GetQueryHelper()
            .map_err(|err| SourceFailure::new(hresult_detail("QueryHelper", &err)))?;
        let value = helper
            .ConnectionString()
            .map_err(|err| SourceFailure::new(hresult_detail("ConnectionString", &err)))?;
        let text = pwstr_string(&value);
        CoTaskMemFree(Some(value.0.cast()));
        text
    };
    if text.is_empty() {
        Err(SourceFailure::new("连接字符串为空"))
    } else {
        Ok(text)
    }
}

fn hresult_detail(action: &str, err: &windows::core::Error) -> String {
    format!("{action} 失败，hresult=0x{:08X}", err.code().0 as u32)
}

/// `VARIANT` 自己的 `Drop` 会 `VariantClear`。这里不再清第二次。
struct VariantBox(VARIANT);

fn variant_bstr(text: &str) -> VariantBox {
    let mut value = VARIANT::default();
    // SAFETY: 新建的空 VARIANT。bstrVal 交给 VariantClear，不在这里再释放。
    unsafe {
        let inner = &mut value.Anonymous.Anonymous;
        inner.vt = VT_BSTR;
        inner.Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(text));
    }
    VariantBox(value)
}

fn variant_i4(n: i32) -> VariantBox {
    let mut value = VARIANT::default();
    // SAFETY: 新建的空 VARIANT，只写入整数。
    unsafe {
        let inner = &mut value.Anonymous.Anonymous;
        inner.vt = VT_I4;
        inner.Anonymous.lVal = n;
    }
    VariantBox(value)
}

fn variant_missing() -> VariantBox {
    let mut value = VARIANT::default();
    // SAFETY: 新建的空 VARIANT，表示可选参数缺省。
    unsafe {
        let inner = &mut value.Anonymous.Anonymous;
        inner.vt = VT_ERROR;
        inner.Anonymous.scode = DISP_E_PARAMNOTFOUND;
    }
    VariantBox(value)
}

fn call(
    disp: &IDispatch,
    name: &str,
    flags: DISPATCH_FLAGS,
    args: &mut [VariantBox],
) -> Result<VariantBox, SourceFailure> {
    let id = dispid(disp, name)?;
    // rgvarg[0] 是最右边的参数。mem::take 把所有权交给这次调用，避免 VariantClear 两次。
    let mut owned: Vec<VARIANT> = args.iter_mut().map(|item| mem::take(&mut item.0)).collect();
    let mut params = DISPPARAMS {
        rgvarg: if owned.is_empty() {
            std::ptr::null_mut()
        } else {
            owned.as_mut_ptr()
        },
        rgdispidNamedArgs: std::ptr::null_mut(),
        cArgs: u32::try_from(owned.len()).unwrap_or(0),
        cNamedArgs: 0,
    };
    let result = invoke(disp, id, flags, &mut params)?;
    for (item, value) in args.iter_mut().zip(owned) {
        item.0 = value;
    }
    Ok(result)
}

fn put_property(disp: &IDispatch, name: &str, value: &mut VARIANT) -> Result<(), SourceFailure> {
    let id = dispid(disp, name)?;
    let mut named = DISPID_PROPERTYPUT;
    let mut params = DISPPARAMS {
        rgvarg: value,
        rgdispidNamedArgs: &mut named,
        cArgs: 1,
        cNamedArgs: 1,
    };
    let _ = invoke(disp, id, DISPATCH_PROPERTYPUT, &mut params)?;
    Ok(())
}

fn invoke(
    disp: &IDispatch,
    id: i32,
    flags: DISPATCH_FLAGS,
    params: &mut DISPPARAMS,
) -> Result<VariantBox, SourceFailure> {
    let mut result = VARIANT::default();
    let mut excep = EXCEPINFO::default();
    let mut argerr = 0u32;
    // SAFETY: 参数和结果都是本函数的局部变量。异常信息里的 BSTR 在下面释放一次。
    let outcome = unsafe {
        disp.Invoke(
            id,
            &GUID::from_u128(0),
            0,
            flags,
            params,
            Some(&mut result),
            Some(&mut excep),
            Some(&mut argerr),
        )
    };
    let description = clear_excep(&mut excep);
    match outcome {
        Ok(()) => Ok(VariantBox(result)),
        Err(err) => {
            drop(result);
            let code = err.code().0 as u32;
            let mut detail = format!("hresult=0x{code:08X}");
            if !description.is_empty() {
                detail.push(' ');
                detail.push_str(&description);
            }
            Err(SourceFailure::new(detail))
        }
    }
}

fn dispid(disp: &IDispatch, name: &str) -> Result<i32, SourceFailure> {
    let wide = wide_null(name);
    let names = [pcwstr(&wide)];
    let mut id = 0i32;
    // SAFETY: 名字以 0 结尾。id 是本函数的局部变量。
    unsafe {
        disp.GetIDsOfNames(&GUID::from_u128(0), names.as_ptr(), 1, 0, &mut id)
            .map_err(|err| SourceFailure::new(hresult_detail(name, &err)))?;
    }
    Ok(id)
}

fn eof(recordset: &IDispatch) -> Result<bool, SourceFailure> {
    let value = get_property(recordset, "EOF")?;
    Ok(variant_bool(&value.0))
}

fn get_property(disp: &IDispatch, name: &str) -> Result<VariantBox, SourceFailure> {
    let id = dispid(disp, name)?;
    let mut params = DISPPARAMS::default();
    invoke(disp, id, DISPATCH_PROPERTYGET, &mut params)
}

fn field_string(recordset: &IDispatch, index: i32) -> Result<String, SourceFailure> {
    let fields = get_property(recordset, "Fields")?;
    let fields = variant_dispatch(&fields.0)?;
    let mut args = [variant_i4(index)];
    let item = call(
        &fields,
        "Item",
        DISPATCH_METHOD | DISPATCH_PROPERTYGET,
        &mut args,
    )?;
    let field = variant_dispatch(&item.0)?;
    let value = get_property(&field, "Value")?;
    Ok(variant_string(&value.0))
}

fn variant_dispatch(value: &VARIANT) -> Result<IDispatch, SourceFailure> {
    // SAFETY: VARIANT 仍由调用方持有。只克隆 IDispatch，不清除原值。
    unsafe {
        let inner = &value.Anonymous.Anonymous;
        if inner.vt != VT_DISPATCH {
            return Err(SourceFailure::new(format!(
                "期望 IDispatch，实际 vt={}",
                inner.vt.0
            )));
        }
        (*inner.Anonymous.pdispVal)
            .clone()
            .ok_or_else(|| SourceFailure::new("IDispatch 为空"))
    }
}

fn variant_bool(value: &VARIANT) -> bool {
    // SAFETY: 只读 vt 和对应的值，不清除。
    unsafe {
        let inner = &value.Anonymous.Anonymous;
        match inner.vt {
            VT_BOOL => inner.Anonymous.boolVal.0 != 0,
            VT_I4 => inner.Anonymous.lVal != 0,
            _ => false,
        }
    }
}

fn variant_string(value: &VARIANT) -> String {
    // SAFETY: 只读字符串。VT_BSTR 的内容由 VARIANT 自己释放。
    unsafe {
        let inner = &value.Anonymous.Anonymous;
        match inner.vt {
            VT_BSTR => inner.Anonymous.bstrVal.to_string(),
            VT_NULL | VT_EMPTY => String::new(),
            other => format!("(vt {})", other.0),
        }
    }
}

fn clear_excep(excep: &mut EXCEPINFO) -> String {
    // SAFETY: EXCEPINFO 的三个 BSTR 由调用方分配，这里各取出一次并丢掉。
    let (description, source, help) = unsafe {
        (
            ManuallyDrop::take(&mut excep.bstrDescription),
            ManuallyDrop::take(&mut excep.bstrSource),
            ManuallyDrop::take(&mut excep.bstrHelpFile),
        )
    };
    let text = if description.is_empty() {
        String::new()
    } else {
        description.to_string()
    };
    drop(description);
    drop(source);
    drop(help);
    text
}

fn pwstr_string(value: &windows::core::PWSTR) -> String {
    if value.is_null() {
        return String::new();
    }
    // SAFETY: 指针非空，且在 CoTaskMemFree 之前有效。
    unsafe { value.to_string() }.unwrap_or_default()
}
