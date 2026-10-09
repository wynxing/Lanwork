//! 文件来源后端。切换逻辑只看见这个接口，测试用假后端。

use super::model::{EverythingStatus, FileHit, WindowsSearchStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProbeNote {
    pub kind: ProbeNoteKind,
    pub line: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) enum ProbeNoteKind {
    /// DLL 不在程序目录，也不在开发构建的仓库目录。
    DllMissing,
    /// 固定版本的 DLL 没有按预期连上。按未运行处理，日志里带版本。
    ProbeFailed,
    /// Windows Search 不可用。日志给诊断，不作为界面说明。
    WindowsSearchDown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EverythingProbe {
    pub status: EverythingStatus,
    pub notes: Vec<ProbeNote>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WindowsSearchProbe {
    pub status: WindowsSearchStatus,
    pub notes: Vec<ProbeNote>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SourceFailure {
    pub detail: String,
}

pub(crate) trait SourceBackend {
    fn probe_everything(&mut self) -> EverythingProbe;
    fn probe_windows_search(&mut self) -> WindowsSearchProbe;
    fn query_everything(&mut self, text: &str) -> Result<Vec<FileHit>, SourceFailure>;
    fn query_windows_search(&mut self, text: &str) -> Result<Vec<FileHit>, SourceFailure>;
}
