//! 密钥只放在宿主侧。Lua 没有读取函数。
//!
//! 产品在凭据管理器里的目标名字符串仍待定。安全测试用
//! `Lanwork.Spike.ToolHost/<工具 id>/<密钥 id>`，只为这次读写，不作为产品规则。
//! 那次写入是当前登录会话的凭据（`CRED_PERSIST_SESSION`），测试结束会删掉。
//! 持久放在哪一级，规格没有写，待定。
//!
//! 隐藏的凭据验证入口用另一套前缀 `Lanwork.ToolHost.Verify/`，写成本机凭据并留在凭据管理器里，
//! 见 [`crate::probe`]。那也不是产品目标名。这个模块不打开数据目录。

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

/// 凭据写到哪一级。产品规格没有定持久级别。安全测试用会话级，验证入口用本机级。
#[cfg(windows)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialPersist {
    /// 当前登录会话。注销后不保留。
    Session,
    /// 这台计算机上的后续登录仍在。凭据管理器里能看到。
    LocalMachine,
}

#[cfg(windows)]
pub use wincred::{credential_delete, credential_list_targets, credential_write};

#[cfg(windows)]
mod wincred {
    use super::{CredentialPersist, SecretStore, spike_target_name};
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::Security::Credentials::{
        CRED_FLAGS, CRED_PERSIST, CRED_PERSIST_LOCAL_MACHINE, CRED_PERSIST_SESSION,
        CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredEnumerateW, CredFree, CredReadW,
        CredWriteW,
    };
    use windows::core::{PCWSTR, PWSTR};

    pub struct WindowsCredentialStore;

    impl SecretStore for WindowsCredentialStore {
        fn put(&mut self, tool_id: &str, secret_id: &str, value: &str) -> Result<(), String> {
            credential_write(
                &spike_target_name(tool_id, secret_id),
                value,
                CredentialPersist::Session,
            )
        }

        fn read(&self, tool_id: &str, secret_id: &str) -> Result<Option<String>, String> {
            credential_read(&spike_target_name(tool_id, secret_id))
        }

        fn remove(&mut self, tool_id: &str, secret_id: &str) -> Result<(), String> {
            credential_delete(&spike_target_name(tool_id, secret_id))
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn persist_flag(persist: CredentialPersist) -> CRED_PERSIST {
        match persist {
            CredentialPersist::Session => CRED_PERSIST_SESSION,
            CredentialPersist::LocalMachine => CRED_PERSIST_LOCAL_MACHINE,
        }
    }

    pub fn credential_write(
        target: &str,
        value: &str,
        persist: CredentialPersist,
    ) -> Result<(), String> {
        let mut target_w = wide(target);
        let mut user = wide("Lanwork");
        let mut blob = value.as_bytes().to_vec();
        let cred = CREDENTIALW {
            Flags: CRED_FLAGS(0),
            Type: CRED_TYPE_GENERIC,
            TargetName: PWSTR(target_w.as_mut_ptr()),
            Comment: PWSTR::null(),
            LastWritten: FILETIME::default(),
            CredentialBlobSize: u32::try_from(blob.len()).map_err(|_| "密钥过长".to_string())?,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: persist_flag(persist),
            AttributeCount: 0,
            Attributes: std::ptr::null_mut(),
            TargetAlias: PWSTR::null(),
            UserName: PWSTR(user.as_mut_ptr()),
        };
        unsafe { CredWriteW(&cred, 0) }.map_err(|err| format!("凭据写入失败：{err}"))
    }

    pub fn credential_read(target: &str) -> Result<Option<String>, String> {
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

    pub fn credential_delete(target: &str) -> Result<(), String> {
        let target_w = wide(target);
        match unsafe { CredDeleteW(PCWSTR(target_w.as_ptr()), CRED_TYPE_GENERIC, Some(0)) } {
            Ok(()) => Ok(()),
            Err(err) if credential_missing(&err) => Ok(()),
            Err(err) => Err(format!("凭据删除失败：{err}")),
        }
    }

    /// 列出目标名。不返回密钥原文。
    pub fn credential_list_targets(prefix: &str) -> Result<Vec<String>, String> {
        // Filter 是目标名前缀，不是通配符。
        let filter = wide(prefix);
        let mut count = 0u32;
        let mut raw: *mut *mut CREDENTIALW = std::ptr::null_mut();
        // `None` 是标志 0。`CRED_ENUMERATE_FLAGS` 不是整数，不能写 `Some(0)`。
        let listed = unsafe { CredEnumerateW(PCWSTR(filter.as_ptr()), None, &mut count, &mut raw) };
        if let Err(err) = listed {
            if credential_missing(&err) {
                return Ok(Vec::new());
            }
            return Err(format!("凭据枚举失败：{err}"));
        }
        if raw.is_null() || count == 0 {
            if !raw.is_null() {
                unsafe { CredFree(raw.cast::<std::ffi::c_void>()) };
            }
            return Ok(Vec::new());
        }
        let creds = unsafe { std::slice::from_raw_parts(raw, count as usize) };
        let mut names = Vec::new();
        let mut failed = None;
        for cred in creds.iter().copied() {
            if cred.is_null() {
                continue;
            }
            let name = unsafe { (*cred).TargetName };
            if name.is_null() {
                continue;
            }
            match unsafe { name.to_string() } {
                Ok(text) if text.starts_with(prefix) => names.push(text),
                Ok(_) => {}
                Err(err) => {
                    failed = Some(format!("目标名无效：{err}"));
                    break;
                }
            }
        }
        unsafe { CredFree(raw.cast::<std::ffi::c_void>()) };
        if let Some(err) = failed {
            return Err(err);
        }
        names.sort();
        Ok(names)
    }

    fn credential_missing(err: &windows::core::Error) -> bool {
        let code = err.code().0 as u32;
        // ERROR_NOT_FOUND 是 1168。Win32 错误进 HRESULT 后是 0x80070490。
        code == 1168 || code == 0x8007_0490
    }
}

#[cfg(windows)]
pub use wincred::WindowsCredentialStore;
