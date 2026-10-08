//! Windows Search 文件名查询。
//!
//! spike 当前用 ADO + `Search.CollatorDSO`，SQL 含 `System.FileName LIKE`。
//! 子串、前缀或整名，以及两套通配符是否同义，产品规格还没定。
//! `ISearchQueryHelper::GenerateSQLFromUserQuery` 和正文 `CONTAINS` 只用于对照。

use std::mem::{self, ManuallyDrop};

use serde::Serialize;
use windows::Win32::System::Com::{
    CLSCTX_ALL, CLSCTX_INPROC_SERVER, CLSIDFromProgID, CoCreateInstance, CoTaskMemFree,
    DISPATCH_FLAGS, DISPATCH_METHOD, DISPATCH_PROPERTYGET, DISPATCH_PROPERTYPUT, DISPPARAMS,
    EXCEPINFO, IDispatch,
};
use windows::Win32::System::Search::{
    CSearchManager, IEnumSearchRoots, ISearchManager, ISearchQueryHelper,
    SEARCH_ADVANCED_QUERY_SYNTAX, SEARCH_TERM_NO_EXPANSION,
};
use windows::Win32::System::Variant::{
    VARIANT, VT_BOOL, VT_BSTR, VT_DISPATCH, VT_EMPTY, VT_ERROR, VT_I4, VT_NULL,
};
use windows::core::{BSTR, GUID, PCWSTR, w};

use crate::filename_sql::{
    aqs_filename, content_contains_sql, filename_like_sql, sql_is_filename_only,
};
use crate::host::{ComInit, FileIdxError, mono_ns, wide_null};
use crate::state::clamp_limit;

const DISPID_PROPERTYPUT: i32 = -3;
const DISP_E_PARAMNOTFOUND: i32 = 0x8002_0004_u32 as i32;
const AD_CMD_TEXT: i32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WsearchMode {
    FilenameLike,
    HelperFilename,
    HelperContentProperties,
    HelperDefault,
    ContentContains,
}

#[derive(Debug, Clone, Serialize)]
pub struct WsearchHitReport {
    pub mode: WsearchMode,
    pub ok: bool,
    pub hresult: Option<u32>,
    pub message: String,
    pub sql: String,
    pub filename_only: bool,
    pub elapsed_ns: Option<u64>,
    pub returned: usize,
    pub limit: usize,
    pub expect_hit: Option<bool>,
    pub reject_hit: Option<bool>,
    pub matching_names: Vec<String>,
    pub connection_string: String,
}

pub struct WsearchSession {
    conn: IDispatch,
    connection_string: String,
    /// 字段按声明顺序析构。放在最后，连接先 `Release`，然后才 `CoUninitialize`。
    _com: ComInit,
}

impl WsearchSession {
    pub fn open() -> Result<Self, FileIdxError> {
        let com = ComInit::new()?;
        let connection_string = helper_connection_string().unwrap_or_else(|_| {
            "Provider=Search.CollatorDSO.1;Extended Properties=\"Application=Windows\"".to_string()
        });
        let clsid = unsafe { CLSIDFromProgID(w!("ADODB.Connection")) }?;
        let conn: IDispatch = unsafe { CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER) }?;
        let mut timeout = variant_i4(20);
        put_property(&conn, "CommandTimeout", &mut timeout.0)?;
        let mut args = [
            variant_missing(),
            variant_missing(),
            variant_missing(),
            variant_bstr(&connection_string),
        ];
        call(&conn, "Open", DISPATCH_METHOD, &mut args)?;
        Ok(Self {
            conn,
            connection_string,
            _com: com,
        })
    }

    pub fn connection_string(&self) -> &str {
        &self.connection_string
    }

    pub fn query(
        &self,
        mode: WsearchMode,
        text: &str,
        limit: usize,
        expect: Option<&str>,
        reject: Option<&str>,
    ) -> WsearchHitReport {
        let limit = clamp_limit(limit);
        let built = match build_sql(mode, text, limit) {
            Ok(sql) => sql,
            Err(err) => {
                return fail_report(mode, limit, String::new(), &self.connection_string, err);
            }
        };
        let start = mono_ns();
        match self.execute(&built.sql) {
            Ok(names) => {
                let elapsed = mono_ns().saturating_sub(start);
                success_report(
                    mode,
                    built.sql,
                    &self.connection_string,
                    elapsed,
                    names,
                    limit,
                    expect,
                    reject,
                )
            }
            Err(err) => {
                let mut report = fail_report(mode, limit, built.sql, &self.connection_string, err);
                report.elapsed_ns = Some(mono_ns().saturating_sub(start));
                report
            }
        }
    }

    fn execute(&self, sql: &str) -> Result<Vec<String>, FileIdxError> {
        let mut args = [
            variant_i4(AD_CMD_TEXT),
            variant_missing(),
            variant_bstr(sql),
        ];
        let result = call(&self.conn, "Execute", DISPATCH_METHOD, &mut args)?;
        let recordset = variant_dispatch(&result.0)?;
        let mut names = Vec::new();
        loop {
            if eof(&recordset)? {
                break;
            }
            if names.len() >= crate::MAX_RESULTS {
                break;
            }
            names.push(field_string(&recordset, 1)?);
            call(&recordset, "MoveNext", DISPATCH_METHOD, &mut [])?;
        }
        let _ = call(&recordset, "Close", DISPATCH_METHOD, &mut []);
        Ok(names)
    }
}

impl Drop for WsearchSession {
    fn drop(&mut self) {
        let _ = call(&self.conn, "Close", DISPATCH_METHOD, &mut []);
    }
}

struct BuiltSql {
    sql: String,
}

fn build_sql(mode: WsearchMode, text: &str, limit: usize) -> Result<BuiltSql, FileIdxError> {
    let sql = match mode {
        WsearchMode::FilenameLike => filename_like_sql(text, limit),
        WsearchMode::ContentContains => content_contains_sql(text, limit),
        WsearchMode::HelperFilename => helper_sql(&aqs_filename(text), limit, None)?,
        WsearchMode::HelperContentProperties => helper_sql(text, limit, Some("System.FileName"))?,
        WsearchMode::HelperDefault => helper_sql(text, limit, None)?,
    };
    Ok(BuiltSql { sql })
}

fn helper_sql(
    aqs: &str,
    limit: usize,
    content_properties: Option<&str>,
) -> Result<String, FileIdxError> {
    let helper = query_helper()?;
    unsafe {
        helper.SetQuerySyntax(SEARCH_ADVANCED_QUERY_SYNTAX)?;
        helper.SetQueryTermExpansion(SEARCH_TERM_NO_EXPANSION)?;
        helper.SetQueryMaxResults(i32::try_from(clamp_limit(limit)).unwrap_or(50))?;
        helper.SetQuerySelectColumns(w!(
            "System.ItemPathDisplay,System.FileName,System.ItemNameDisplay,System.ItemType"
        ))?;
        if let Some(props) = content_properties {
            let wide = wide_null(props);
            helper.SetQueryContentProperties(PCWSTR(wide.as_ptr()))?;
        }
        let wide = wide_null(aqs);
        let sql = helper.GenerateSQLFromUserQuery(PCWSTR(wide.as_ptr()))?;
        let text = pwstr_string(&sql);
        CoTaskMemFree(Some(sql.0.cast()));
        Ok(text)
    }
}

fn query_helper() -> Result<ISearchQueryHelper, FileIdxError> {
    unsafe {
        // SearchIndexer 注册为 LocalServer32。只请求进程内会得到 0x80040154。
        let manager: ISearchManager = CoCreateInstance(&CSearchManager, None, CLSCTX_ALL)?;
        let catalog = manager.GetCatalog(w!("SystemIndex"))?;
        Ok(catalog.GetQueryHelper()?)
    }
}

fn helper_connection_string() -> Result<String, FileIdxError> {
    let helper = query_helper()?;
    unsafe {
        let value = helper.ConnectionString()?;
        let text = pwstr_string(&value);
        CoTaskMemFree(Some(value.0.cast()));
        Ok(text)
    }
}

pub fn indexed_roots() -> Result<Vec<String>, FileIdxError> {
    let _com = ComInit::new()?;
    unsafe {
        // 与 query_helper 相同：目录管理器在本机服务进程里。
        let manager: ISearchManager = CoCreateInstance(&CSearchManager, None, CLSCTX_ALL)?;
        let catalog = manager.GetCatalog(w!("SystemIndex"))?;
        let scope = catalog.GetCrawlScopeManager()?;
        let roots: IEnumSearchRoots = scope.EnumerateRoots()?;
        let mut urls = Vec::new();
        loop {
            let mut item = [None];
            let mut fetched = 0u32;
            if roots.Next(&mut item, &mut fetched).is_err() || fetched == 0 {
                break;
            }
            if let Some(root) = item[0].take() {
                let url = root.RootURL()?;
                urls.push(pwstr_string(&url));
                CoTaskMemFree(Some(url.0.cast()));
            }
        }
        Ok(urls)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ManagerContextReport {
    pub context: &'static str,
    pub ok: bool,
    pub hresult: Option<u32>,
    pub message: String,
}

/// 对照 `CSearchManager` 在两种上下文里能否创建。结果原样记下，不改写 HRESULT。
pub fn probe_search_manager() -> Result<Vec<ManagerContextReport>, FileIdxError> {
    let _com = ComInit::new()?;
    Ok(vec![
        create_manager("inproc_server", CLSCTX_INPROC_SERVER),
        create_manager("all", CLSCTX_ALL),
    ])
}

fn create_manager(
    context: &'static str,
    ctx: windows::Win32::System::Com::CLSCTX,
) -> ManagerContextReport {
    match unsafe { CoCreateInstance::<_, ISearchManager>(&CSearchManager, None, ctx) } {
        Ok(manager) => {
            drop(manager);
            ManagerContextReport {
                context,
                ok: true,
                hresult: None,
                message: String::new(),
            }
        }
        Err(err) => ManagerContextReport {
            context,
            ok: false,
            hresult: Some(err.code().0 as u32),
            message: err.message().to_string(),
        },
    }
}

pub fn query_windows(
    mode: WsearchMode,
    text: &str,
    limit: usize,
    expect: Option<&str>,
    reject: Option<&str>,
) -> Result<WsearchHitReport, FileIdxError> {
    let session = WsearchSession::open()?;
    Ok(session.query(mode, text, limit, expect, reject))
}

pub fn compare_modes(
    text: &str,
    limit: usize,
    expect: Option<&str>,
    reject: Option<&str>,
) -> Result<Vec<WsearchHitReport>, FileIdxError> {
    let session = WsearchSession::open()?;
    let modes = [
        WsearchMode::FilenameLike,
        WsearchMode::HelperFilename,
        WsearchMode::HelperContentProperties,
        WsearchMode::HelperDefault,
        WsearchMode::ContentContains,
    ];
    Ok(modes
        .into_iter()
        .map(|mode| session.query(mode, text, limit, expect, reject))
        .collect())
}

#[allow(clippy::too_many_arguments)]
fn success_report(
    mode: WsearchMode,
    sql: String,
    connection_string: &str,
    elapsed: u64,
    names: Vec<String>,
    limit: usize,
    expect: Option<&str>,
    reject: Option<&str>,
) -> WsearchHitReport {
    let expect_hit =
        expect.map(|needle| names.iter().any(|name| name.eq_ignore_ascii_case(needle)));
    let reject_hit =
        reject.map(|needle| names.iter().any(|name| name.eq_ignore_ascii_case(needle)));
    let matching_names = names
        .iter()
        .filter(|name| {
            expect.is_some_and(|needle| name.eq_ignore_ascii_case(needle))
                || reject.is_some_and(|needle| name.eq_ignore_ascii_case(needle))
        })
        .cloned()
        .collect();
    WsearchHitReport {
        mode,
        ok: true,
        hresult: None,
        message: String::new(),
        filename_only: sql_is_filename_only(&sql),
        sql,
        elapsed_ns: Some(elapsed),
        returned: names.len(),
        limit,
        expect_hit,
        reject_hit,
        matching_names,
        connection_string: connection_string.to_string(),
    }
}

fn fail_report(
    mode: WsearchMode,
    limit: usize,
    sql: String,
    connection_string: &str,
    err: FileIdxError,
) -> WsearchHitReport {
    WsearchHitReport {
        mode,
        ok: false,
        hresult: err.hresult,
        message: err.message,
        filename_only: sql_is_filename_only(&sql),
        sql,
        elapsed_ns: None,
        returned: 0,
        limit,
        expect_hit: None,
        reject_hit: None,
        matching_names: Vec::new(),
        connection_string: connection_string.to_string(),
    }
}

/// `VARIANT` 自己的 `Drop` 会 `VariantClear`。这里不再清第二次。
struct VariantBox(VARIANT);

fn variant_bstr(text: &str) -> VariantBox {
    let mut value = VARIANT::default();
    unsafe {
        let inner = &mut value.Anonymous.Anonymous;
        inner.vt = VT_BSTR;
        inner.Anonymous.bstrVal = ManuallyDrop::new(BSTR::from(text));
    }
    VariantBox(value)
}

fn variant_i4(n: i32) -> VariantBox {
    let mut value = VARIANT::default();
    unsafe {
        let inner = &mut value.Anonymous.Anonymous;
        inner.vt = VT_I4;
        inner.Anonymous.lVal = n;
    }
    VariantBox(value)
}

fn variant_missing() -> VariantBox {
    let mut value = VARIANT::default();
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
) -> Result<VariantBox, FileIdxError> {
    let id = dispid(disp, name)?;
    // 调用方按 IDispatch 的顺序放入参数：rgvarg[0] 是最右边的参数。
    // `mem::take` 把所有权交给这次调用。`ptr::read` 会留下第二份 `VARIANT`，
    // 两边的 `Drop` 都会 `VariantClear`，BSTR 会被释放两次。
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
    let result = invoke(disp, id, flags, &mut params, None)?;
    for (item, value) in args.iter_mut().zip(owned) {
        item.0 = value;
    }
    Ok(result)
}

fn put_property(disp: &IDispatch, name: &str, value: &mut VARIANT) -> Result<(), FileIdxError> {
    let id = dispid(disp, name)?;
    let mut named = DISPID_PROPERTYPUT;
    let mut params = DISPPARAMS {
        rgvarg: value,
        rgdispidNamedArgs: &mut named,
        cArgs: 1,
        cNamedArgs: 1,
    };
    let _ = invoke(disp, id, DISPATCH_PROPERTYPUT, &mut params, None)?;
    Ok(())
}

fn invoke(
    disp: &IDispatch,
    id: i32,
    flags: DISPATCH_FLAGS,
    params: &mut DISPPARAMS,
    _named: Option<()>,
) -> Result<VariantBox, FileIdxError> {
    let mut result = VARIANT::default();
    let mut excep = EXCEPINFO::default();
    let mut argerr = 0u32;
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
            let mut message = err.message().to_string();
            if !description.is_empty() {
                message.push_str(": ");
                message.push_str(&description);
            }
            Err(FileIdxError::with_hresult(message, err.code().0 as u32))
        }
    }
}

fn dispid(disp: &IDispatch, name: &str) -> Result<i32, FileIdxError> {
    let wide = wide_null(name);
    let names = [PCWSTR(wide.as_ptr())];
    let mut id = 0i32;
    unsafe {
        disp.GetIDsOfNames(&GUID::from_u128(0), names.as_ptr(), 1, 0, &mut id)
            .map_err(FileIdxError::from)?;
    }
    Ok(id)
}

fn eof(recordset: &IDispatch) -> Result<bool, FileIdxError> {
    let value = get_property(recordset, "EOF")?;
    Ok(variant_bool(&value.0))
}

fn get_property(disp: &IDispatch, name: &str) -> Result<VariantBox, FileIdxError> {
    let id = dispid(disp, name)?;
    let mut params = DISPPARAMS::default();
    invoke(disp, id, DISPATCH_PROPERTYGET, &mut params, None)
}

fn field_string(recordset: &IDispatch, index: i32) -> Result<String, FileIdxError> {
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

fn variant_dispatch(value: &VARIANT) -> Result<IDispatch, FileIdxError> {
    unsafe {
        let inner = &value.Anonymous.Anonymous;
        if inner.vt != VT_DISPATCH {
            return Err(FileIdxError::new(format!(
                "期望 IDispatch，实际 vt={}",
                inner.vt.0
            )));
        }
        (*inner.Anonymous.pdispVal)
            .clone()
            .ok_or_else(|| FileIdxError::new("IDispatch 为空"))
    }
}

fn variant_bool(value: &VARIANT) -> bool {
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
    let description = unsafe { ManuallyDrop::take(&mut excep.bstrDescription) };
    let text = if description.is_empty() {
        String::new()
    } else {
        description.to_string()
    };
    drop(description);
    drop(unsafe { ManuallyDrop::take(&mut excep.bstrSource) });
    drop(unsafe { ManuallyDrop::take(&mut excep.bstrHelpFile) });
    text
}

fn pwstr_string(value: &windows::core::PWSTR) -> String {
    if value.is_null() {
        return String::new();
    }
    unsafe { value.to_string() }.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_search_query_returns_a_report() {
        let report = query_windows(
            WsearchMode::FilenameLike,
            "lanwork-fileidx-unit-absent-token",
            5,
            None,
            None,
        );
        match report {
            Ok(report) => {
                assert!(report.returned <= 5);
                if report.ok {
                    assert!(report.filename_only);
                    assert!(report.sql.contains("System.FileName"));
                } else {
                    assert!(report.hresult.is_some() || !report.message.is_empty());
                }
            }
            Err(err) => {
                assert!(!err.to_string().is_empty());
            }
        }
    }

    #[test]
    fn variant_take_clears_a_bstr_once() {
        let mut boxed = variant_bstr("lanwork-fileidx");
        let taken = mem::take(&mut boxed.0);
        drop(boxed);
        drop(taken);
    }

    #[test]
    fn session_releases_the_connection_before_uninitializing_com() {
        // 连接字段在 COM 守卫前面。连续打开再丢掉，Release 发生在 CoUninitialize 之前。
        let mut opened = 0;
        for _ in 0..4 {
            match WsearchSession::open() {
                Ok(session) => {
                    let _ = session.query(
                        WsearchMode::FilenameLike,
                        "lanwork-fileidx-unit-absent-token",
                        1,
                        None,
                        None,
                    );
                    drop(session);
                    opened += 1;
                }
                Err(err) => {
                    assert!(!err.to_string().is_empty());
                    return;
                }
            }
        }
        assert!(opened >= 1);
    }
}
