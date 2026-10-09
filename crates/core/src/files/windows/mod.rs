//! Windows 上的 Everything 与 Windows Search。不创建窗口。

mod com;
mod everything;
mod locate;
mod wide;
mod wsearch;

use everything::{LoadFail, Sdk3, Sdk14, SdkStatus};
use locate::DllRoots;
use wsearch::Wsearch;

use crate::files::backend::{
    EverythingProbe, ProbeNote, ProbeNoteKind, SourceBackend, SourceFailure, WindowsSearchProbe,
};
use crate::files::model::{EverythingStatus, FileHit};
use crate::files::service::{dll_missing_line, probe_failed_line};

pub(crate) struct Host {
    roots: DllRoots,
    sdk3: Option<Sdk3>,
    sdk14: Option<Sdk14>,
    selected: Selected,
    search: Wsearch,
}

enum Selected {
    Sdk3,
    Sdk14,
    None,
}

impl Host {
    pub(crate) fn from_process() -> Self {
        Self {
            roots: DllRoots::from_process(),
            sdk3: None,
            sdk14: None,
            selected: Selected::None,
            search: Wsearch::new(),
        }
    }

    fn ensure_sdk3(&mut self) -> Result<(), LoadFail> {
        if self.sdk3.is_some() {
            return Ok(());
        }
        let path = self.roots.sdk3().map_err(|_| LoadFail::Missing)?;
        self.sdk3 = Some(Sdk3::load(&path)?);
        Ok(())
    }

    fn ensure_sdk14(&mut self) -> Result<(), LoadFail> {
        if self.sdk14.is_some() {
            return Ok(());
        }
        let path = self.roots.sdk14().map_err(|_| LoadFail::Missing)?;
        self.sdk14 = Some(Sdk14::load(&path)?);
        Ok(())
    }
}

impl SourceBackend for Host {
    fn probe_everything(&mut self) -> EverythingProbe {
        let mut notes = Vec::new();
        self.selected = Selected::None;
        match self.ensure_sdk3() {
            Ok(()) => {
                let view = self.sdk3.as_mut().expect("sdk3").probe();
                if let Some(note) = view.note {
                    notes.push(note);
                }
                match view.status {
                    SdkStatus::Ready => {
                        self.selected = Selected::Sdk3;
                        return EverythingProbe {
                            status: EverythingStatus::Ready,
                            notes,
                        };
                    }
                    SdkStatus::NotReady => {
                        return EverythingProbe {
                            status: EverythingStatus::NotReady,
                            notes,
                        };
                    }
                    SdkStatus::NotRunning => {}
                }
            }
            Err(fail) => notes.push(load_note("sdk3", "Everything3_x64.dll", fail)),
        }
        match self.ensure_sdk14() {
            Ok(()) => {
                let view = self.sdk14.as_mut().expect("sdk14").probe();
                if let Some(note) = view.note {
                    notes.push(note);
                }
                let status = match view.status {
                    SdkStatus::Ready => {
                        self.selected = Selected::Sdk14;
                        EverythingStatus::Ready
                    }
                    SdkStatus::NotReady => EverythingStatus::NotReady,
                    SdkStatus::NotRunning => EverythingStatus::NotRunning,
                };
                EverythingProbe { status, notes }
            }
            Err(fail) => {
                notes.push(load_note("sdk14", "Everything64.dll", fail));
                EverythingProbe {
                    status: EverythingStatus::NotRunning,
                    notes,
                }
            }
        }
    }

    fn probe_windows_search(&mut self) -> WindowsSearchProbe {
        self.search.probe()
    }

    fn query_everything(&mut self, text: &str) -> Result<Vec<FileHit>, SourceFailure> {
        match self.selected {
            Selected::Sdk3 => self.sdk3.as_mut().expect("sdk3").query(text),
            Selected::Sdk14 => self.sdk14.as_mut().expect("sdk14").query(text),
            Selected::None => Err(SourceFailure::new("Everything 未连接")),
        }
    }

    fn query_windows_search(&mut self, text: &str) -> Result<Vec<FileHit>, SourceFailure> {
        self.search.query(text)
    }
}

fn load_note(sdk: &str, file_name: &str, fail: LoadFail) -> ProbeNote {
    match fail {
        LoadFail::Missing => ProbeNote {
            kind: ProbeNoteKind::DllMissing,
            line: dll_missing_line(file_name),
        },
        LoadFail::Failed(detail) => ProbeNote {
            kind: ProbeNoteKind::ProbeFailed,
            line: probe_failed_line(sdk, "未知", &detail),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::Host;
    use crate::files::model::{EverythingStatus, FileSource, MAX_FILE_RESULTS};
    use crate::files::service::{Engine, FileLog};

    struct Silent;

    impl FileLog for Silent {
        fn error(&self, _message: &str) {}

        fn warn(&self, _message: &str) {}
    }

    #[test]
    fn absent_token_stays_within_the_cap_and_does_not_invent_a_label() {
        let mut engine = Engine::new(Host::from_process(), Silent);
        let result = engine.query(9, "lanwork-core-file-absent-9f3a2c");
        assert_eq!(result.sequence, 9);
        assert!(result.hits.len() <= MAX_FILE_RESULTS);
        assert_ne!(result.everything, EverythingStatus::NotChecked);
        match result.source {
            FileSource::Everything | FileSource::WindowsSearch => {
                assert_eq!(result.unavailable_label(), None);
            }
            FileSource::Unavailable => {
                assert!(result.hits.is_empty());
                assert_eq!(result.unavailable_label(), Some("文件索引不可用"));
            }
            FileSource::Blank => panic!("查询词不是空白"),
        }
    }
}
