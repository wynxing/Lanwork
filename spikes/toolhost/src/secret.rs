//! 密钥只放在宿主侧。Lua 没有读取函数。
//!
//! 产品在凭据管理器里的目标名字符串仍待定。Windows 自动测试用
//! `Lanwork.Spike.ToolHost/<工具 id>/<密钥 id>`，只为这次读写，不作为产品规则。
//! 写入的是当前登录会话的凭据（`CRED_PERSIST_SESSION`），进程结束并注销后不保留。
//! 持久放在哪一级，规格没有写，待定。
//!
//! Win32 读写只在 `cfg(test)` 里编译：唯一调用方是 Windows 上的凭据往返测试。
//! 非测试的库目标不包含这段代码。命令行验证用 `MemorySecretStore`。

use std::collections::BTreeMap;

pub fn spike_target_name(tool_id: &str, secret_id: &str) -> String {
    format!("Lanwork.Spike.ToolHost/{tool_id}/{secret_id}")
}

pub trait SecretStore {
    fn put(&mut self, tool_id: &str, secret_id: &str, value: &str) -> Result<(), String>;
    fn read(&self, tool_id: &str, secret_id: &str) -> Result<Option<String>, String>;
    fn remove(&mut self, tool_id: &str, secret_id: &str) -> Result<(), String>;
}

#[derive(Default)]
pub struct MemorySecretStore {
    values: BTreeMap<(String, String), String>,
}

impl SecretStore for MemorySecretStore {
    fn put(&mut self, tool_id: &str, secret_id: &str, value: &str) -> Result<(), String> {
        self.values.insert(
            (tool_id.to_string(), secret_id.to_string()),
            value.to_string(),
        );
        Ok(())
    }

    fn read(&self, tool_id: &str, secret_id: &str) -> Result<Option<String>, String> {
        Ok(self
            .values
            .get(&(tool_id.to_string(), secret_id.to_string()))
            .cloned())
    }

    fn remove(&mut self, tool_id: &str, secret_id: &str) -> Result<(), String> {
        self.values
            .remove(&(tool_id.to_string(), secret_id.to_string()));
        Ok(())
    }
}

#[cfg(all(windows, test))]
mod wincred {
    use super::{SecretStore, spike_target_name};
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::Security::Credentials::{
        CRED_FLAGS, CRED_PERSIST_SESSION, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredFree,
        CredReadW, CredWriteW,
    };
    use windows::core::{PCWSTR, PWSTR};

    pub struct WindowsCredentialStore;

    impl SecretStore for WindowsCredentialStore {
        fn put(&mut self, tool_id: &str, secret_id: &str, value: &str) -> Result<(), String> {
            write_secret(&spike_target_name(tool_id, secret_id), value)
        }

        fn read(&self, tool_id: &str, secret_id: &str) -> Result<Option<String>, String> {
            read_secret(&spike_target_name(tool_id, secret_id))
        }

        fn remove(&mut self, tool_id: &str, secret_id: &str) -> Result<(), String> {
            delete_secret(&spike_target_name(tool_id, secret_id))
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn write_secret(target: &str, value: &str) -> Result<(), String> {
        let mut target_w = wide(target);
        let mut user = wide("lanwork-toolhost-spike");
        let mut blob = value.as_bytes().to_vec();
        let cred = CREDENTIALW {
            Flags: CRED_FLAGS(0),
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target_w.as_mut_ptr()),
            Comment: PWSTR::null(),
            LastWritten: FILETIME::default(),
            CredentialBlobSize: u32::try_from(blob.len()).map_err(|_| "密钥过长".to_string())?,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_SESSION,
            AttributeCount: 0,
            Attributes: std::ptr::null_mut(),
            TargetAlias: PWSTR::null(),
            UserName: PWSTR(user.as_mut_ptr()),
        };
        unsafe { CredWriteW(&cred, 0) }.map_err(|err| format!("凭据写入失败：{err}"))
    }

    fn read_secret(target: &str) -> Result<Option<String>, String> {
        let target_w = wide(target);
        let mut raw = std::ptr::null_mut();
        let read = unsafe {
            CredReadW(
                PCWSTR(target_w.as_ptr()),
                CRED_TYPE_GENERIC,
                Some(0),
                &mut raw,
            )
        };
        if let Err(err) = read {
            if credential_missing(&err) {
                return Ok(None);
            }
            return Err(format!("凭据读取失败：{err}"));
        }
        if raw.is_null() {
            return Ok(None);
        }
        let cred = unsafe { &*raw };
        let bytes = unsafe {
            std::slice::from_raw_parts(cred.CredentialBlob, cred.CredentialBlobSize as usize)
        };
        let text = String::from_utf8(bytes.to_vec());
        unsafe { CredFree(raw.cast::<std::ffi::c_void>()) };
        match text {
            Ok(value) => Ok(Some(value)),
            Err(_) => Err("凭据不是 UTF-8".to_string()),
        }
    }

    fn delete_secret(target: &str) -> Result<(), String> {
        let target_w = wide(target);
        match unsafe { CredDeleteW(PCWSTR(target_w.as_ptr()), CRED_TYPE_GENERIC, Some(0)) } {
            Ok(()) => Ok(()),
            Err(err) if credential_missing(&err) => Ok(()),
            Err(err) => Err(format!("凭据删除失败：{err}")),
        }
    }

    fn credential_missing(err: &windows::core::Error) -> bool {
        let code = err.code().0 as u32;
        // ERROR_NOT_FOUND 是 1168。Win32 错误进 HRESULT 后是 0x80070490。
        code == 1168 || code == 0x8007_0490
    }
}

#[cfg(all(windows, test))]
pub use wincred::WindowsCredentialStore;
