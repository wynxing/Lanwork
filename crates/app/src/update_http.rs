//! WinHTTP。只访问 `lanwork_core::update` 允许的 https 地址，自己跟重定向。
//!
//! 不关闭证书校验，也不打开自动解压。正文边读边交给调用方，超过上限就停。

use std::ptr::{self, NonNull};

use lanwork_core::update::{
    GetBytes, MAX_REDIRECTS, TransportError, current_version, parse_https_url, resolve_redirect,
    url_allowed,
};
use windows::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ADDREQ_FLAG_ADD, WINHTTP_DISABLE_REDIRECTS,
    WINHTTP_FLAG_SECURE, WINHTTP_OPTION_DISABLE_FEATURE, WINHTTP_OPTION_REDIRECT_POLICY,
    WINHTTP_OPTION_REDIRECT_POLICY_NEVER, WINHTTP_QUERY_CONTENT_LENGTH, WINHTTP_QUERY_FLAG_NUMBER,
    WINHTTP_QUERY_LOCATION, WINHTTP_QUERY_STATUS_CODE, WinHttpAddRequestHeaders,
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryHeaders,
    WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetOption,
    WinHttpSetTimeouts,
};
use windows::core::{HSTRING, PCWSTR};

const RESOLVE_MS: i32 = 10_000;
const CONNECT_MS: i32 = 15_000;
const SEND_MS: i32 = 30_000;
const RECEIVE_MS: i32 = 60_000;
const CHUNK: usize = 64 * 1024;

pub(crate) struct WinHttp;

impl GetBytes for WinHttp {
    fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>, TransportError> {
        let mut body = Vec::new();
        pull(
            url,
            limit,
            "application/vnd.github+json",
            |chunk, _total| {
                body.extend_from_slice(chunk);
                Ok(())
            },
        )?;
        Ok(body)
    }
}

/// 把最终的 2xx 正文分块交给 `on_chunk`。`total` 是 Content-Length，没有就是 `None`。
pub(crate) fn pull(
    url: &str,
    limit: u64,
    accept: &str,
    mut on_chunk: impl FnMut(&[u8], Option<u64>) -> Result<(), TransportError>,
) -> Result<(), TransportError> {
    let mut current = url.to_owned();
    for hop in 0..=MAX_REDIRECTS {
        if !url_allowed(&current) {
            return Err(TransportError::Network);
        }
        let Some(parsed) = parse_https_url(&current) else {
            return Err(TransportError::Network);
        };
        let opened = open_request(&parsed.host, parsed.port, &parsed.path_and_query, accept)?;
        let status = status_code(opened.request.as_raw())?;
        if (300..400).contains(&status) {
            if hop == MAX_REDIRECTS {
                return Err(TransportError::Network);
            }
            let location = header_text(opened.request.as_raw(), WINHTTP_QUERY_LOCATION)?;
            current = resolve_redirect(&current, &location).ok_or(TransportError::Network)?;
            continue;
        }
        if !(200..300).contains(&status) {
            return Err(TransportError::Http(status));
        }
        let total = content_length(opened.request.as_raw())?;
        if let Some(total) = total
            && total > limit
        {
            return Err(TransportError::TooLarge);
        }
        // `opened` 活到这次读取结束，句柄才关掉。
        return read_body(opened.request.as_raw(), limit, total, &mut on_chunk);
    }
    Err(TransportError::Network)
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
    fn new(raw: *mut std::ffi::c_void) -> Result<Self, TransportError> {
        NonNull::new(raw).map(Self).ok_or(TransportError::Network)
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

fn open_request(host: &str, port: u16, path: &str, accept: &str) -> Result<Opened, TransportError> {
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
    let request = Internet::new(unsafe {
        WinHttpOpenRequest(
            connection.as_raw(),
            &method,
            &path,
            PCWSTR::null(),
            PCWSTR::null(),
            ptr::null(),
            WINHTTP_FLAG_SECURE,
        )
    })?;
    disable_redirects(request.as_raw())?;
    let header = accept_header(accept)?;
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

fn accept_header(accept: &str) -> Result<Vec<u16>, TransportError> {
    if accept
        .chars()
        .any(|ch| ch.is_control() || ch == '\r' || ch == '\n')
    {
        return Err(TransportError::Network);
    }
    let text = format!("Accept: {accept}\r\n");
    Ok(text.encode_utf16().chain(std::iter::once(0)).collect())
}

fn disable_redirects(request: *mut std::ffi::c_void) -> Result<(), TransportError> {
    let disable = WINHTTP_DISABLE_REDIRECTS.to_ne_bytes();
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

fn status_code(request: *mut std::ffi::c_void) -> Result<u16, TransportError> {
    let mut code = 0u32;
    let mut size =
        u32::try_from(std::mem::size_of::<u32>()).map_err(|_| TransportError::Network)?;
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
    u16::try_from(code).map_err(|_| TransportError::Network)
}

fn content_length(request: *mut std::ffi::c_void) -> Result<Option<u64>, TransportError> {
    let mut len = 0u32;
    let mut size =
        u32::try_from(std::mem::size_of::<u32>()).map_err(|_| TransportError::Network)?;
    let result = unsafe {
        WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_CONTENT_LENGTH | WINHTTP_QUERY_FLAG_NUMBER,
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

fn header_text(request: *mut std::ffi::c_void, level: u32) -> Result<String, TransportError> {
    let mut buffer_len = 0u32;
    let _ = unsafe {
        WinHttpQueryHeaders(
            request,
            level,
            PCWSTR::null(),
            None,
            &mut buffer_len,
            ptr::null_mut(),
        )
    };
    if buffer_len == 0 {
        return Err(TransportError::Network);
    }
    let units = usize::try_from(buffer_len / 2).map_err(|_| TransportError::Network)? + 1;
    let mut buffer = vec![0u16; units];
    map_win(unsafe {
        WinHttpQueryHeaders(
            request,
            level,
            PCWSTR::null(),
            Some(buffer.as_mut_ptr() as *mut _),
            &mut buffer_len,
            ptr::null_mut(),
        )
    })?;
    let count = usize::try_from(buffer_len / 2).unwrap_or(0);
    let count = count.min(buffer.len());
    Ok(String::from_utf16_lossy(&buffer[..count]))
}

fn read_body(
    request: *mut std::ffi::c_void,
    limit: u64,
    total: Option<u64>,
    on_chunk: &mut impl FnMut(&[u8], Option<u64>) -> Result<(), TransportError>,
) -> Result<(), TransportError> {
    let mut buf = vec![0u8; CHUNK];
    let to_read = u32::try_from(buf.len()).map_err(|_| TransportError::Network)?;
    let mut received = 0u64;
    loop {
        let mut read = 0u32;
        map_win(unsafe {
            WinHttpReadData(request, buf.as_mut_ptr() as *mut _, to_read, &mut read)
        })?;
        if read == 0 {
            break;
        }
        let count = usize::try_from(read).map_err(|_| TransportError::Network)?;
        let count = count.min(buf.len());
        received = received.saturating_add(count as u64);
        if received > limit {
            return Err(TransportError::TooLarge);
        }
        on_chunk(&buf[..count], total)?;
    }
    Ok(())
}

fn map_win(result: windows::core::Result<()>) -> Result<(), TransportError> {
    result.map_err(|_| TransportError::Network)
}
