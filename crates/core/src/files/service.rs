//! 每次查询前检查状态，再决定走 Everything 还是 Windows Search。
//!
//! 不保留查询队列。序号原样带回，过期序号由查询调度丢掉。

use std::collections::BTreeSet;

use super::backend::{ProbeNoteKind, SourceBackend, SourceFailure};
use super::model::{
    EverythingStatus, FileHit, FileQueryResult, FileSource, MAX_FILE_RESULTS, WindowsSearchStatus,
    is_blank_query,
};

pub(crate) trait FileLog {
    fn error(&self, message: &str);
    fn warn(&self, message: &str);
}

pub(crate) struct Engine<B, L> {
    backend: B,
    log: L,
    logged: BTreeSet<String>,
}

impl<B, L> Engine<B, L>
where
    B: SourceBackend,
    L: FileLog,
{
    pub(crate) fn new(backend: B, log: L) -> Self {
        Self {
            backend,
            log,
            logged: BTreeSet::new(),
        }
    }

    pub(crate) fn query(&mut self, sequence: u64, text: &str) -> FileQueryResult {
        if is_blank_query(text) {
            return FileQueryResult {
                sequence,
                everything: EverythingStatus::NotChecked,
                windows_search: WindowsSearchStatus::NotChecked,
                source: FileSource::Blank,
                hits: Vec::new(),
            };
        }

        let everything = self.backend.probe_everything();
        self.write_notes(&everything.notes);
        if everything.status == EverythingStatus::Ready {
            match self.backend.query_everything(text) {
                Ok(hits) => {
                    return ready_result(sequence, hits);
                }
                Err(failure) => {
                    // 检查时还是就绪，查询调用却失败了。规格没有写这一步。
                    // 当前按未运行，本次继续问 Windows Search。
                    self.log_once(
                        &format!(
                            "Everything 查询失败，本次改查 Windows Search：{}",
                            failure.detail
                        ),
                        true,
                    );
                }
            }
        }

        let windows_search = self.backend.probe_windows_search();
        self.write_notes(&windows_search.notes);
        if windows_search.status == WindowsSearchStatus::Available {
            match self.backend.query_windows_search(text) {
                Ok(hits) => {
                    return FileQueryResult {
                        sequence,
                        everything: fallen_back(everything.status),
                        windows_search: WindowsSearchStatus::Available,
                        source: FileSource::WindowsSearch,
                        hits: cap(hits),
                    };
                }
                Err(failure) => {
                    self.log_once(&format!("Windows Search 不可用：{}", failure.detail), true);
                    return unavailable(sequence, fallen_back(everything.status));
                }
            }
        }

        unavailable(sequence, fallen_back(everything.status))
    }

    fn write_notes(&mut self, notes: &[super::backend::ProbeNote]) {
        for note in notes {
            let error = matches!(
                note.kind,
                ProbeNoteKind::DllMissing | ProbeNoteKind::ProbeFailed
            );
            self.log_once(&note.line, error);
        }
    }

    fn log_once(&mut self, line: &str, error: bool) {
        if !self.logged.insert(line.to_owned()) {
            return;
        }
        if error {
            self.log.error(line);
        } else {
            self.log.warn(line);
        }
    }
}

fn ready_result(sequence: u64, hits: Vec<FileHit>) -> FileQueryResult {
    FileQueryResult {
        sequence,
        everything: EverythingStatus::Ready,
        windows_search: WindowsSearchStatus::NotChecked,
        source: FileSource::Everything,
        hits: cap(hits),
    }
}

fn fallen_back(status: EverythingStatus) -> EverythingStatus {
    match status {
        EverythingStatus::Ready => EverythingStatus::NotRunning,
        other => other,
    }
}

fn unavailable(sequence: u64, everything: EverythingStatus) -> FileQueryResult {
    FileQueryResult {
        sequence,
        everything,
        windows_search: WindowsSearchStatus::Unavailable,
        source: FileSource::Unavailable,
        hits: Vec::new(),
    }
}

fn cap(mut hits: Vec<FileHit>) -> Vec<FileHit> {
    if hits.len() > MAX_FILE_RESULTS {
        hits.truncate(MAX_FILE_RESULTS);
    }
    hits
}

pub(crate) fn dll_missing_line(path: &str) -> String {
    format!("Everything DLL 缺失：{path}")
}

#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn probe_failed_line(sdk: &str, version: &str, detail: &str) -> String {
    format!("Everything 探测失败，按未运行处理。sdk={sdk} 版本={version} {detail}")
}

pub(crate) fn windows_search_down_line(detail: &str) -> String {
    format!("Windows Search 不可用：{detail}")
}

impl SourceFailure {
    pub(crate) fn new(detail: impl Into<String>) -> Self {
        Self {
            detail: detail.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::backend::{
        EverythingProbe, ProbeNote, ProbeNoteKind, SourceFailure, WindowsSearchProbe,
    };
    use crate::files::model::{FileKind, FileSource};
    use std::sync::Mutex;

    struct MemLog {
        lines: Mutex<Vec<String>>,
    }

    impl MemLog {
        fn new() -> Self {
            Self {
                lines: Mutex::new(Vec::new()),
            }
        }

        fn lines(&self) -> Vec<String> {
            self.lines.lock().expect("log").clone()
        }
    }

    impl FileLog for &MemLog {
        fn error(&self, message: &str) {
            self.lines.lock().expect("log").push(message.to_owned());
        }

        fn warn(&self, message: &str) {
            self.lines.lock().expect("log").push(message.to_owned());
        }
    }

    #[derive(Clone)]
    struct HitSpec {
        name: &'static str,
        folder: bool,
    }

    struct Fake {
        everything: Vec<EverythingStatus>,
        windows: Vec<WindowsSearchStatus>,
        everything_hits: Vec<HitSpec>,
        windows_hits: Vec<HitSpec>,
        everything_error: bool,
        windows_error: bool,
        notes: Vec<ProbeNote>,
        calls: Vec<&'static str>,
    }

    impl Fake {
        fn status(everything: EverythingStatus, windows: WindowsSearchStatus) -> Self {
            Self {
                everything: vec![everything],
                windows: vec![windows],
                everything_hits: vec![HitSpec {
                    name: "from-everything.txt",
                    folder: false,
                }],
                windows_hits: vec![HitSpec {
                    name: "from-search.txt",
                    folder: true,
                }],
                everything_error: false,
                windows_error: false,
                notes: Vec::new(),
                calls: Vec::new(),
            }
        }

        fn take_status(items: &mut Vec<EverythingStatus>) -> EverythingStatus {
            if items.len() > 1 {
                items.remove(0)
            } else {
                items[0]
            }
        }
    }

    impl SourceBackend for Fake {
        fn probe_everything(&mut self) -> EverythingProbe {
            self.calls.push("probe-everything");
            EverythingProbe {
                status: Self::take_status(&mut self.everything),
                notes: self.notes.clone(),
            }
        }

        fn probe_windows_search(&mut self) -> WindowsSearchProbe {
            self.calls.push("probe-wsearch");
            let status = if self.windows.len() > 1 {
                self.windows.remove(0)
            } else {
                self.windows[0]
            };
            WindowsSearchProbe {
                status,
                notes: Vec::new(),
            }
        }

        fn query_everything(&mut self, text: &str) -> Result<Vec<FileHit>, SourceFailure> {
            self.calls.push("query-everything");
            let _ = text;
            if self.everything_error {
                return Err(SourceFailure::new("ipc"));
            }
            Ok(hits(&self.everything_hits))
        }

        fn query_windows_search(&mut self, text: &str) -> Result<Vec<FileHit>, SourceFailure> {
            self.calls.push("query-wsearch");
            let _ = text;
            if self.windows_error {
                return Err(SourceFailure::new("hresult=0x80020009"));
            }
            Ok(hits(&self.windows_hits))
        }
    }

    fn hits(specs: &[HitSpec]) -> Vec<FileHit> {
        specs
            .iter()
            .map(|spec| FileHit {
                name: spec.name.to_owned(),
                path: format!("C:\\probe\\{}", spec.name),
                kind: if spec.folder {
                    FileKind::Folder
                } else {
                    FileKind::File
                },
            })
            .collect()
    }

    fn run(fake: Fake, text: &str) -> (FileQueryResult, Vec<&'static str>, Vec<String>) {
        let log = MemLog::new();
        let mut engine = Engine::new(fake, &log);
        let result = engine.query(7, text);
        let calls = engine.backend.calls.clone();
        (result, calls, log.lines())
    }

    #[test]
    fn ready_everything_is_used_and_windows_search_is_not_asked() {
        let (result, calls, _) = run(
            Fake::status(EverythingStatus::Ready, WindowsSearchStatus::Available),
            "报告",
        );
        assert_eq!(result.sequence, 7);
        assert_eq!(result.source, FileSource::Everything);
        assert_eq!(result.hits[0].name, "from-everything.txt");
        assert_eq!(result.unavailable_label(), None);
        assert_eq!(calls, ["probe-everything", "query-everything"]);
    }

    #[test]
    fn not_running_and_not_ready_use_windows_search() {
        for status in [EverythingStatus::NotRunning, EverythingStatus::NotReady] {
            let (result, calls, _) =
                run(Fake::status(status, WindowsSearchStatus::Available), "报告");
            assert_eq!(result.source, FileSource::WindowsSearch);
            assert_eq!(result.everything, status);
            assert_eq!(result.hits[0].name, "from-search.txt");
            assert_eq!(result.hits[0].kind, FileKind::Folder);
            assert!(calls.contains(&"query-wsearch"));
            assert!(!calls.contains(&"query-everything"));
        }
    }

    #[test]
    fn next_query_returns_to_everything_when_it_is_ready_again() {
        let mut fake = Fake::status(EverythingStatus::NotReady, WindowsSearchStatus::Available);
        fake.everything = vec![EverythingStatus::NotReady, EverythingStatus::Ready];
        let log = MemLog::new();
        let mut engine = Engine::new(fake, &log);
        let first = engine.query(1, "a");
        let second = engine.query(2, "a");
        assert_eq!(first.source, FileSource::WindowsSearch);
        assert_eq!(second.sequence, 2);
        assert_eq!(second.source, FileSource::Everything);
        assert_eq!(second.hits[0].name, "from-everything.txt");
    }

    #[test]
    fn both_unavailable_reports_the_specified_label_and_no_hits() {
        let (result, calls, _) = run(
            Fake::status(
                EverythingStatus::NotRunning,
                WindowsSearchStatus::Unavailable,
            ),
            "报告",
        );
        assert_eq!(result.source, FileSource::Unavailable);
        assert!(result.hits.is_empty());
        assert_eq!(result.unavailable_label(), Some("文件索引不可用"));
        assert_eq!(calls, ["probe-everything", "probe-wsearch"]);
    }

    #[test]
    fn windows_search_query_failure_is_unavailable_not_an_empty_success() {
        let mut fake = Fake::status(EverythingStatus::NotRunning, WindowsSearchStatus::Available);
        fake.windows_error = true;
        let (result, _, lines) = run(fake, "notepad");
        assert_eq!(result.source, FileSource::Unavailable);
        assert!(result.hits.is_empty());
        assert_eq!(result.unavailable_label(), Some("文件索引不可用"));
        assert!(lines.iter().any(|line| line.contains("0x80020009")));
        assert!(!lines.iter().any(|line| line.contains("文件索引不可用")));
    }

    #[test]
    fn missing_dll_is_logged_once_and_search_still_runs() {
        let mut fake = Fake::status(EverythingStatus::NotRunning, WindowsSearchStatus::Available);
        fake.notes.push(ProbeNote {
            kind: ProbeNoteKind::DllMissing,
            line: dll_missing_line("Everything3_x64.dll"),
        });
        let log = MemLog::new();
        let mut engine = Engine::new(fake, &log);
        let first = engine.query(1, "a");
        let second = engine.query(2, "b");
        assert_eq!(first.source, FileSource::WindowsSearch);
        assert_eq!(second.source, FileSource::WindowsSearch);
        let lines = log.lines();
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.contains("Everything DLL 缺失"))
                .count(),
            1
        );
    }

    #[test]
    fn probe_failure_logs_the_version_and_is_not_running() {
        let mut fake = Fake::status(
            EverythingStatus::NotRunning,
            WindowsSearchStatus::Unavailable,
        );
        fake.notes.push(ProbeNote {
            kind: ProbeNoteKind::ProbeFailed,
            line: probe_failed_line("sdk3", "1.5.0.1423", "last_error=0xE0000099"),
        });
        let (result, _, lines) = run(fake, "a");
        assert_eq!(result.everything, EverythingStatus::NotRunning);
        assert_eq!(result.source, FileSource::Unavailable);
        assert!(
            lines.iter().any(|line| {
                line.contains("探测失败") && line.contains("版本=1.5.0.1423")
            })
        );
    }

    #[test]
    fn everything_query_error_falls_through_to_windows_search() {
        let mut fake = Fake::status(EverythingStatus::Ready, WindowsSearchStatus::Available);
        fake.everything_error = true;
        let (result, calls, lines) = run(fake, "a");
        assert_eq!(result.source, FileSource::WindowsSearch);
        assert_eq!(result.everything, EverythingStatus::NotRunning);
        assert_eq!(result.hits[0].name, "from-search.txt");
        assert_eq!(
            calls,
            [
                "probe-everything",
                "query-everything",
                "probe-wsearch",
                "query-wsearch"
            ]
        );
        assert!(lines.iter().any(|line| line.contains("查询失败")));
    }

    #[test]
    fn blank_input_does_not_probe() {
        let (result, calls, _) = run(
            Fake::status(EverythingStatus::Ready, WindowsSearchStatus::Available),
            "  \n",
        );
        assert_eq!(result.source, FileSource::Blank);
        assert!(result.hits.is_empty());
        assert_eq!(result.unavailable_label(), None);
        assert!(calls.is_empty());
    }

    #[test]
    fn hits_are_capped_at_50() {
        let mut fake = Fake::status(EverythingStatus::Ready, WindowsSearchStatus::Available);
        fake.everything_hits = (0..80)
            .map(|_| HitSpec {
                name: "cap.txt",
                folder: false,
            })
            .collect();
        let (result, _, _) = run(fake, "cap");
        assert_eq!(result.hits.len(), 50);
    }

    #[test]
    fn successful_windows_search_with_no_hits_is_not_unavailable() {
        let mut fake = Fake::status(EverythingStatus::NotRunning, WindowsSearchStatus::Available);
        fake.windows_hits.clear();
        let (result, _, _) = run(fake, "absent-token");
        assert_eq!(result.source, FileSource::WindowsSearch);
        assert!(result.hits.is_empty());
        assert_eq!(result.unavailable_label(), None);
    }
}
