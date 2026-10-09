//! 非 Windows 上没有文件索引。状态按两者都不可用，不查询。

use super::backend::{
    EverythingProbe, ProbeNote, ProbeNoteKind, SourceBackend, SourceFailure, WindowsSearchProbe,
};
use super::model::{EverythingStatus, FileHit, WindowsSearchStatus};
use super::service::{dll_missing_line, windows_search_down_line};

#[derive(Debug)]
pub(crate) struct Host;

impl SourceBackend for Host {
    fn probe_everything(&mut self) -> EverythingProbe {
        EverythingProbe {
            status: EverythingStatus::NotRunning,
            notes: vec![
                ProbeNote {
                    kind: ProbeNoteKind::DllMissing,
                    line: dll_missing_line("Everything3_x64.dll"),
                },
                ProbeNote {
                    kind: ProbeNoteKind::DllMissing,
                    line: dll_missing_line("Everything64.dll"),
                },
            ],
        }
    }

    fn probe_windows_search(&mut self) -> WindowsSearchProbe {
        WindowsSearchProbe {
            status: WindowsSearchStatus::Unavailable,
            notes: vec![ProbeNote {
                kind: ProbeNoteKind::WindowsSearchDown,
                line: windows_search_down_line("只在 Windows 上查询"),
            }],
        }
    }

    fn query_everything(&mut self, _text: &str) -> Result<Vec<FileHit>, SourceFailure> {
        Err(SourceFailure::new("只在 Windows 上查询"))
    }

    fn query_windows_search(&mut self, _text: &str) -> Result<Vec<FileHit>, SourceFailure> {
        Err(SourceFailure::new("只在 Windows 上查询"))
    }
}
