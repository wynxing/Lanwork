//! 文件来源的薄命令。
//!
//! 界面以后只调用这里。这里不打开窗口，也不合并应用、待办和便签。

use std::sync::{Arc, Mutex};

use crate::storage::{self, Store};

use super::backend::SourceBackend;
use super::model::FileQueryResult;
use super::service::{Engine, FileLog};

/// 界面使用的文件查询。克隆后仍是同一份后端和同一把锁。
#[derive(Clone)]
pub struct FileCommands {
    inner: Arc<Mutex<EngineBox>>,
}

struct EngineBox {
    engine: Box<dyn QueryEngine>,
}

trait QueryEngine: Send {
    fn query(&mut self, sequence: u64, text: &str) -> FileQueryResult;
}

struct Erased<B, L> {
    engine: Engine<B, L>,
}

impl<B, L> QueryEngine for Erased<B, L>
where
    B: SourceBackend + Send + 'static,
    L: FileLog + Send + 'static,
{
    fn query(&mut self, sequence: u64, text: &str) -> FileQueryResult {
        self.engine.query(sequence, text)
    }
}

impl std::fmt::Debug for FileCommands {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileCommands").finish_non_exhaustive()
    }
}

impl FileCommands {
    #[must_use]
    pub fn open(store: Store) -> Self {
        Self::from_backend(process_backend(), StoreLog(store))
    }

    /// 查询文件和文件夹。`sequence` 原样带回。最多 50 条。
    pub fn query(&self, sequence: u64, text: &str) -> FileQueryResult {
        let mut guard = storage::lock_mutex(&self.inner);
        guard.engine.query(sequence, text)
    }

    fn from_backend<B, L>(backend: B, log: L) -> Self
    where
        B: SourceBackend + Send + 'static,
        L: FileLog + Send + 'static,
    {
        Self {
            inner: Arc::new(Mutex::new(EngineBox {
                engine: Box::new(Erased {
                    engine: Engine::new(backend, log),
                }),
            })),
        }
    }
}

struct StoreLog(Store);

impl FileLog for StoreLog {
    fn error(&self, message: &str) {
        self.0.log_error(message);
    }

    fn warn(&self, message: &str) {
        self.0.log_warn(message);
    }
}

#[cfg(windows)]
fn process_backend() -> super::windows::Host {
    super::windows::Host::from_process()
}

#[cfg(not(windows))]
fn process_backend() -> super::stub::Host {
    super::stub::Host
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::backend::{
        EverythingProbe, ProbeNote, ProbeNoteKind, SourceBackend, SourceFailure, WindowsSearchProbe,
    };
    use crate::files::model::{EverythingStatus, FileSource, WindowsSearchStatus};
    use crate::files::service::dll_missing_line;
    use crate::storage::StorePaths;
    use crate::storage::test_temp::TempDir;

    struct MissingDll;

    impl SourceBackend for MissingDll {
        fn probe_everything(&mut self) -> EverythingProbe {
            EverythingProbe {
                status: EverythingStatus::NotRunning,
                notes: vec![ProbeNote {
                    kind: ProbeNoteKind::DllMissing,
                    line: dll_missing_line("Everything64.dll"),
                }],
            }
        }

        fn probe_windows_search(&mut self) -> WindowsSearchProbe {
            WindowsSearchProbe {
                status: WindowsSearchStatus::Unavailable,
                notes: Vec::new(),
            }
        }

        fn query_everything(
            &mut self,
            _text: &str,
        ) -> Result<Vec<crate::files::FileHit>, SourceFailure> {
            Err(SourceFailure::new("未运行"))
        }

        fn query_windows_search(
            &mut self,
            _text: &str,
        ) -> Result<Vec<crate::files::FileHit>, SourceFailure> {
            Err(SourceFailure::new("不可用"))
        }
    }

    #[test]
    fn missing_dll_is_written_to_the_app_log() {
        let temp = TempDir::new();
        let store = Store::open(StorePaths {
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            user_profile: temp.path().join("profile"),
            local_app_data: temp.path().join("local"),
        })
        .unwrap();
        let commands = FileCommands::from_backend(MissingDll, StoreLog(store.clone()));
        let result = commands.query(4, "报告");
        assert_eq!(result.sequence, 4);
        assert_eq!(result.source, FileSource::Unavailable);
        assert_eq!(result.unavailable_label(), Some("文件索引不可用"));
        let log = std::fs::read_to_string(store.log_path()).unwrap();
        assert!(log.contains("Everything DLL 缺失：Everything64.dll"));
        assert!(!log.contains("文件索引不可用"));
    }
}
