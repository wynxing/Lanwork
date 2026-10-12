//! 设置页录入热键的状态。
//!
//! 「录入中」只有一个出口：[`HotkeyRecording::end`]。录到组合、Esc、再点一次、失焦（含刚显示的
//! 宽限期内）、收起、切标签、关闭和进程退出，都用 [`EndReason`] 走它，所以录入结束时
//! 把卸掉的热键注册回去这件事不会漏掉某条路径。注册回去之后没注册上的热键记在这里，
//! 收起面板不清，下次注册上才清。

use super::bind::HotkeyRole;
use super::capture::Recorded;

const INVALID_HOTKEY: &str = "热键无效";
const NO_REPLY: &str = "热键服务没有应答";

/// 录入结束的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    /// 放弃：再点一次、失焦、收起、切标签、关闭、进程退出，以及开始录入时平台线程没应答。
    Abandon,
    /// 按键拦截器已经有结果：录到组合、Esc，或录到不能用的键。
    Recorded(Recorded),
}

/// 结束录入时要交给平台线程做的事：把热键注册回去，再按需保存。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ending {
    /// 录到的组合；`None` 表示只恢复，不保存。
    pub save: Option<String>,
    rejected: bool,
}

/// 平台线程注册回去之后的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndReport {
    /// 录到的搜索条热键的保存结果。
    pub saved: Result<(), String>,
    /// 按配置应当注册、此刻没有注册的热键。
    pub unregistered: Vec<HotkeyRole>,
}

#[derive(Debug, Default)]
pub struct HotkeyRecording {
    active: bool,
    unregistered: Vec<HotkeyRole>,
}

impl HotkeyRecording {
    /// `unregistered` 是启动时按配置没注册上的热键。
    #[must_use]
    pub fn new(unregistered: Vec<HotkeyRole>) -> Self {
        Self {
            active: false,
            unregistered,
        }
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// 进入录入。已经在录入时返回 false。
    pub fn begin(&mut self) -> bool {
        if self.active {
            return false;
        }
        self.active = true;
        true
    }

    /// 离开录入的唯一出口。不在录入时返回 `None`，什么也不用做；否则返回要做的事，并且状态已经
    /// 回到未录入，所以同一次录入只会结束一次。
    pub fn end(&mut self, reason: EndReason) -> Option<Ending> {
        if !self.active {
            return None;
        }
        self.active = false;
        Some(match reason {
            EndReason::Abandon | EndReason::Recorded(Recorded::Cancelled) => Ending {
                save: None,
                rejected: false,
            },
            EndReason::Recorded(Recorded::Captured(hotkey)) => Ending {
                save: Some(hotkey.canonical()),
                rejected: false,
            },
            EndReason::Recorded(Recorded::Rejected) => Ending {
                save: None,
                rejected: true,
            },
        })
    }

    /// 平台线程的结果回来之后调用。返回要显示的错误；没有应答时返回错误并保持原来的「没注册上」。
    pub fn finish(&mut self, ending: &Ending, report: Option<EndReport>) -> Result<(), String> {
        let Some(report) = report else {
            return Err(NO_REPLY.to_owned());
        };
        self.unregistered = report.unregistered;
        match report.saved {
            Ok(()) if ending.rejected => Err(INVALID_HOTKEY.to_owned()),
            other => other,
        }
    }

    /// 配置里有、此刻没注册上的热键。
    #[must_use]
    pub fn unregistered(&self) -> &[HotkeyRole] {
        &self.unregistered
    }

    /// 错误条里留着的热键提示。没有要提示的时为 `None`。
    #[must_use]
    pub fn hotkey_error(&self) -> Option<String> {
        if self.unregistered.is_empty() {
            return None;
        }
        let parts: Vec<String> = self
            .unregistered
            .iter()
            .map(|role| format!("{}已被占用", role.title()))
            .collect();
        Some(parts.join("；"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_hotkey;

    fn report(saved: Result<(), String>, unregistered: &[HotkeyRole]) -> Option<EndReport> {
        Some(EndReport {
            saved,
            unregistered: unregistered.to_vec(),
        })
    }

    fn captured() -> EndReason {
        EndReason::Recorded(Recorded::Captured(parse_hotkey("Ctrl+Alt+K").unwrap()))
    }

    #[test]
    fn every_way_out_of_recording_ends_it_once_and_asks_for_the_hotkeys_back() {
        let reasons = [
            EndReason::Abandon,
            EndReason::Recorded(Recorded::Cancelled),
            EndReason::Recorded(Recorded::Rejected),
            captured(),
        ];
        for reason in reasons {
            let mut state = HotkeyRecording::new(Vec::new());
            assert!(state.begin());
            assert!(state.is_active());
            let ending = state.end(reason);
            assert!(ending.is_some(), "{reason:?}");
            assert!(!state.is_active(), "{reason:?}");
            assert_eq!(state.end(reason), None, "{reason:?}：同一次录入只结束一次");
            assert_eq!(state.end(EndReason::Abandon), None, "{reason:?}");
        }
    }

    #[test]
    fn leaving_without_recording_does_nothing() {
        let mut state = HotkeyRecording::new(Vec::new());
        assert_eq!(state.end(EndReason::Abandon), None);
        assert!(state.begin());
        assert!(!state.begin(), "录入中再开始不重复卸热键");
    }

    #[test]
    fn only_a_captured_combo_is_saved() {
        let mut state = HotkeyRecording::new(Vec::new());
        state.begin();
        assert_eq!(state.end(EndReason::Abandon).unwrap().save, None);
        state.begin();
        let ending = state.end(captured()).unwrap();
        assert_eq!(ending.save.as_deref(), Some("Ctrl+Alt+K"));
        state.begin();
        assert_eq!(
            state
                .end(EndReason::Recorded(Recorded::Cancelled))
                .unwrap()
                .save,
            None
        );
        state.begin();
        assert_eq!(
            state
                .end(EndReason::Recorded(Recorded::Rejected))
                .unwrap()
                .save,
            None
        );
    }

    #[test]
    fn an_unusable_key_reports_invalid_unless_something_worse_happened() {
        let mut state = HotkeyRecording::new(Vec::new());
        state.begin();
        let ending = state.end(EndReason::Recorded(Recorded::Rejected)).unwrap();
        assert_eq!(
            state.finish(&ending, report(Ok(()), &[])),
            Err("热键无效".to_owned())
        );
    }

    #[test]
    fn a_save_error_is_returned_as_is() {
        let mut state = HotkeyRecording::new(Vec::new());
        state.begin();
        let ending = state.end(captured()).unwrap();
        assert_eq!(
            state.finish(&ending, report(Err("热键已被占用".into()), &[])),
            Err("热键已被占用".to_owned())
        );
    }

    #[test]
    fn a_hotkey_that_could_not_be_restored_stays_reported_until_it_registers() {
        let mut state = HotkeyRecording::new(Vec::new());
        state.begin();
        let ending = state.end(EndReason::Abandon).unwrap();
        assert_eq!(
            state.finish(&ending, report(Ok(()), &[HotkeyRole::Panel])),
            Ok(())
        );
        assert_eq!(state.hotkey_error().as_deref(), Some("面板热键已被占用"));

        // 收起面板、再打开：没有新的录入，提示还在。
        assert_eq!(state.end(EndReason::Abandon), None);
        assert_eq!(state.hotkey_error().as_deref(), Some("面板热键已被占用"));

        state.begin();
        let ending = state.end(EndReason::Abandon).unwrap();
        state.finish(&ending, report(Ok(()), &[])).unwrap();
        assert_eq!(state.hotkey_error(), None);
    }

    #[test]
    fn the_startup_failure_is_kept_and_both_roles_are_named() {
        let state = HotkeyRecording::new(vec![HotkeyRole::Search, HotkeyRole::Panel]);
        assert_eq!(
            state.hotkey_error().as_deref(),
            Some("搜索条热键已被占用；面板热键已被占用")
        );
        assert_eq!(HotkeyRecording::new(Vec::new()).hotkey_error(), None);
    }

    #[test]
    fn no_reply_from_the_platform_thread_is_an_error_and_keeps_the_old_report() {
        let mut state = HotkeyRecording::new(vec![HotkeyRole::Search]);
        state.begin();
        let ending = state.end(EndReason::Abandon).unwrap();
        assert_eq!(
            state.finish(&ending, None),
            Err("热键服务没有应答".to_owned())
        );
        assert_eq!(state.unregistered(), [HotkeyRole::Search]);
        assert!(!state.is_active());
    }
}
