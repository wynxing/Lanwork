//! 网站图标的 WinHTTP。只请求 `http` 和 `https`，自己跟有限次重定向。
//!
//! 不关闭证书校验，不带 Cookie，错误里不写地址。

use std::ptr::{self, NonNull};

use lanwork_core::shelves::{
    Fetched, WEB_ICON_MAX_REDIRECTS, parse_icon_url, resolve_icon_redirect,
};
use lanwork_core::update::current_version;
use windows::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ADDREQ_FLAG_ADD, WINHTTP_DISABLE_COOKIES,
    WINHTTP_DISABLE_REDIRECTS, WINHTTP_FLAG_SECURE, WINHTTP_OPTION_DISABLE_FEATURE,
    WINHTTP_OPTION_REDIRECT_POLICY, WINHTTP_OPTION_REDIRECT_POLICY_NEVER,
    WINHTTP_QUERY_CONTENT_TYPE, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_LOCATION,
    WINHTTP_QUERY_STATUS_CODE, WinHttpAddRequestHeaders, WinHttpCloseHandle, WinHttpConnect,
    WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse,
    WinHttpSendRequest, WinHttpSetOption, WinHttpSetTimeouts,
};
use windows::core::{HSTRING, PCWSTR};

const RESOLVE_MS: i32 = 8_000;
const CONNECT_MS: i32 = 8_000;
const SEND_MS: i32 = 10_000;
const RECEIVE_MS: i32 = 10_000;
const CHUNK: usize = 16 * 1024;

/// 取一个图标或页面。失败时返回 `None`，不把地址写进错误。
pub(crate) fn get(url: &str, limit: usize) -> Option<Fetched> {
    let limit = u64::try_from(limit).ok()?;
    pull(url, limit).ok()
}

fn pull(url: &str, limit: u64) -> Result<Fetched, ()> {
    let mut current = parse_icon_url(url).ok_or(())?.as_url();
    for hop in 0..=WEB_ICON_MAX_REDIRECTS {
        let parsed = parse_icon_url(&current).ok_or(())?;
        let opened = open_request(
            &parsed.host,
            parsed.port,
            &parsed.path_and_query,
            parsed.secure,
        )?;
        let status = status_code(opened.request.as_raw())?;
        if (300..400).contains(&status) {
            if hop == WEB_ICON_MAX_REDIRECTS {
                return Err(());
            }
            let location = header_text(opened.request.as_raw(), WINHTTP_QUERY_LOCATION)?;
            current = resolve_icon_redirect(&current, &location).ok_or(())?;
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(());
        }
        let total = content_length(opened.request.as_raw())?;
        if let Some(total) = total
            && total > limit
        {
            return Err(());
        }
        let body = read_body(opened.request.as_raw(), limit, total)?;
        if body.is_empty() {
            return Err(());
        }
        let content_type = header_text(opened.request.as_raw(), WINHTTP_QUERY_CONTENT_TYPE).ok();
        return Ok(Fetched {
            final_url: current,
            content_type,
            body,
        });
    }
    Err(())
}

struct Internet(NonNull<std::ffi::c_void>);

impl Drop for Internet {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0.as_ptr());
        }
    }
}

impl Internet {
    fn new(raw: *mut std::ffi::c_void) -> Result<Self, ()> {
        NonNull::new(raw).map(Self).ok_or(())
    }

    fn as_raw(&self) -> *mut std::ffi::c_void {
        self.0.as_ptr()
    }
}

struct Opened {
    _session: Internet,
    _connection: Internet,
    request: Internet,
}

fn open_request(host: &str, port: u16, path: &str, secure: bool) -> Result<Opened, ()> {
    let agent = HSTRING::from(format!("Lanwork/{}", current_version()));
    let session = Internet::new(unsafe {
        WinHttpOpen(
            &agent,
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        )
    })?;
    map_win(unsafe {
        WinHttpSetTimeouts(
            session.as_raw(),
            RESOLVE_MS,
            CONNECT_MS,
            SEND_MS,
            RECEIVE_MS,
        )
    })?;
    let host = HSTRING::from(host);
    let connection = Internet::new(unsafe { WinHttpConnect(session.as_raw(), &host, port, 0) })?;
    let method = HSTRING::from("GET");
    let path = HSTRING::from(path);
    let flags = if secure {
        WINHTTP_FLAG_SECURE
    } else {
        windows::Win32::Networking::WinHttp::WINHTTP_OPEN_REQUEST_FLAGS(0)
    };
    let request = Internet::new(unsafe {
        WinHttpOpenRequest(
            connection.as_raw(),
            &method,
            &path,
            PCWSTR::null(),
            PCWSTR::null(),
            ptr::null(),
            flags,
        )
    })?;
    disable_redirects_and_cookies(request.as_raw())?;
    let header = accept_header()?;
    map_win(unsafe {
        WinHttpAddRequestHeaders(
            request.as_raw(),
            &header[..header.len() - 1],
            WINHTTP_ADDREQ_FLAG_ADD,
        )
    })?;
    map_win(unsafe { WinHttpSendRequest(request.as_raw(), None, None, 0, 0, 0) })?;
    map_win(unsafe { WinHttpReceiveResponse(request.as_raw(), ptr::null_mut()) })?;
    Ok(Opened {
        _session: session,
        _connection: connection,
        request,
    })
}

fn accept_header() -> Result<Vec<u16>, ()> {
    let text = "Accept: */*\r\n";
    Ok(text.encode_utf16().chain(std::iter::once(0)).collect())
}

fn disable_redirects_and_cookies(request: *mut std::ffi::c_void) -> Result<(), ()> {
    let disable = (WINHTTP_DISABLE_REDIRECTS | WINHTTP_DISABLE_COOKIES).to_ne_bytes();
    map_win(unsafe {
        WinHttpSetOption(
            Some(request.cast_const()),
            WINHTTP_OPTION_DISABLE_FEATURE,
            Some(&disable),
        )
    })?;
    let policy = WINHTTP_OPTION_REDIRECT_POLICY_NEVER.to_ne_bytes();
    map_win(unsafe {
        WinHttpSetOption(
            Some(request.cast_const()),
            WINHTTP_OPTION_REDIRECT_POLICY,
            Some(&policy),
        )
    })
}

fn status_code(request: *mut std::ffi::c_void) -> Result<u16, ()> {
    let mut code = 0u32;
    let mut size = u32::try_from(std::mem::size_of::<u32>()).map_err(|_| ())?;
    map_win(unsafe {
        WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut code as *mut u32 as *mut _),
            &mut size,
            ptr::null_mut(),
        )
    })?;
    u16::try_from(code).map_err(|_| ())
}

fn content_length(request: *mut std::ffi::c_void) -> Result<Option<u64>, ()> {
    let mut len = 0u32;
    let mut size = u32::try_from(std::mem::size_of::<u32>()).map_err(|_| ())?;
    let result = unsafe {
        WinHttpQueryHeaders(
            request,
            windows::Win32::Networking::WinHttp::WINHTTP_QUERY_CONTENT_LENGTH
                | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut len as *mut u32 as *mut _),
            &mut size,
            ptr::null_mut(),
        )
    };
    match result {
        Ok(()) => Ok(Some(u64::from(len))),
        Err(_) => Ok(None),
    }
}

fn header_text(request: *mut std::ffi::c_void, query: u32) -> Result<String, ()> {
    let mut buffer_len = 0u32;
    let _ = unsafe {
        WinHttpQueryHeaders(
            request,
            query,
            PCWSTR::null(),
            None,
            &mut buffer_len,
            ptr::null_mut(),
        )
    };
    if buffer_len == 0 {
        return Err(());
    }
    let units = usize::try_from(buffer_len / 2).map_err(|_| ())? + 1;
    let mut buffer = vec![0u16; units];
    map_win(unsafe {
        WinHttpQueryHeaders(
            request,
            query,
            PCWSTR::null(),
            Some(buffer.as_mut_ptr() as *mut _),
            &mut buffer_len,
            ptr::null_mut(),
        )
    })?;
    let count = usize::try_from(buffer_len / 2).unwrap_or(0);
    let count = count.min(buffer.len());
    let text = String::from_utf16_lossy(&buffer[..count]);
    let text = text.trim_matches(char::from(0)).trim();
    if text.is_empty() {
        Err(())
    } else {
        Ok(text.to_owned())
    }
}

fn read_body(
    request: *mut std::ffi::c_void,
    limit: u64,
    total: Option<u64>,
) -> Result<Vec<u8>, ()> {
    if let Some(total) = total
        && total > limit
    {
        return Err(());
    }
    let mut body = Vec::new();
    let mut buf = vec![0u8; CHUNK];
    let to_read = u32::try_from(buf.len()).map_err(|_| ())?;
    loop {
        let mut read = 0u32;
        map_win(unsafe {
            WinHttpReadData(request, buf.as_mut_ptr() as *mut _, to_read, &mut read)
        })?;
        if read == 0 {
            break;
        }
        let count = usize::try_from(read).map_err(|_| ())?;
        let count = count.min(buf.len());
        if u64::try_from(body.len() + count).unwrap_or(u64::MAX) > limit {
            return Err(());
        }
        body.extend_from_slice(&buf[..count]);
    }
    Ok(body)
}

fn map_win(result: windows::core::Result<()>) -> Result<(), ()> {
    result.map_err(|_| ())
}
