//! Everything SDK 与 Windows Search 文件名查询的技术验证。
//!
//! 不进 `lanwork` 的依赖，也不进发布包。运行说明在 `spikes/fileidx/README.md`。

mod filename_sql;
mod state;

#[cfg(windows)]
mod everything;
#[cfg(windows)]
mod host;
#[cfg(windows)]
mod loadmon;
#[cfg(windows)]
mod wsearch;

pub use filename_sql::{
    aqs_filename, content_contains_sql, filename_like_sql, like_pattern, sql_is_filename_only,
};
pub use state::{
    EVERYTHING_ERROR_IPC, EVERYTHING_OK, EVERYTHING3_ERROR_IPC_PIPE_NOT_FOUND, EVERYTHING3_OK,
    MAX_RESULTS, MachineState, ProbeKind, clamp_limit, classify_sdk3, classify_sdk14,
    machine_state,
};

#[cfg(windows)]
pub use everything::{
    CapCheck, EverythingQuery, InstallSnapshot, PollReport, ProbeReport, SdkAttempt, SdkChoice,
    cap_check, install_snapshot, poll, probe, probe_one, query_everything,
};
#[cfg(windows)]
pub use host::{FileIdxError, mono_ns, private_bytes};
#[cfg(windows)]
pub use loadmon::{LoadSnapshot, sample_load};
#[cfg(windows)]
pub use wsearch::{
    WsearchHitReport, WsearchMode, WsearchSession, compare_modes, indexed_roots, query_windows,
};

#[cfg(not(windows))]
use std::fmt;

#[cfg(not(windows))]
#[derive(Debug)]
pub struct FileIdxError {
    message: String,
}

#[cfg(not(windows))]
impl FileIdxError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

#[cfg(not(windows))]
impl fmt::Display for FileIdxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

#[cfg(not(windows))]
impl std::error::Error for FileIdxError {}
