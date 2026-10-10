//! 便签编辑窗口共用的保存流程：自动保存的计时、关闭前保存、失败和冲突之后的选择。
//!
//! 规则在 `lanwork_core::panel::EditSession`，这里只接上计时器和界面线程。面板的便签页和每个悬浮窗
//! 各持有一个 [`Editor`]。`after` 是保存没有成功时暂存的后续动作（关闭、切换便签、退出），
//! 用户重试成功、放弃修改或选定版本之后再继续。

use std::time::Duration;

use lanwork_core::notes::{Note, NoteCommands};
use lanwork_core::panel::{EditSession, SaveOutcome, SaveProblem};
use slint::SharedString;

use crate::host::unix_now_ms;

pub(crate) struct Editor<A> {
    pub(crate) session: EditSession,
    /// 保存没有成功时等着继续的动作。有值就是「正在关闭」，编辑区据此显示「放弃修改」。
    pub(crate) after: Option<A>,
    timer: slint::Timer,
}

/// 编辑区状态条要的三个值：问题种类（0 没有，1 失败，2 冲突）、错误文字、是否正在关闭。
pub(crate) struct ProblemView {
    pub(crate) kind: i32,
    pub(crate) text: SharedString,
    pub(crate) closing: bool,
}

impl<A> Editor<A> {
    pub(crate) fn new(note: &Note) -> Self {
        Self {
            session: EditSession::new(note),
            after: None,
            timer: slint::Timer::default(),
        }
    }

    /// 改草稿，然后按停止编辑的时间重新排自动保存。`tick` 在到点时调用。
    pub(crate) fn edit(
        &mut self,
        change: impl FnOnce(&mut EditSession, i64),
        tick: impl Fn() + 'static,
    ) {
        let now = unix_now_ms();
        change(&mut self.session, now);
        self.arm(now, tick);
    }

    pub(crate) fn arm(&mut self, now: i64, tick: impl Fn() + 'static) {
        match self.session.autosave_wait_ms(now) {
            Some(wait) => {
                let wait = u64::try_from(wait.max(1)).unwrap_or(1);
                self.timer.start(
                    slint::TimerMode::SingleShot,
                    Duration::from_millis(wait),
                    tick,
                );
            }
            None => self.timer.stop(),
        }
    }

    /// 自动保存的计时到点。输入法还在组合时不保存，再等一个周期；还没到停止编辑 1 秒时按剩余时间再等。
    /// 返回是否做了保存。
    pub(crate) fn on_tick(
        &mut self,
        preedit_empty: bool,
        notes: &NoteCommands,
        tick: impl Fn() + 'static,
    ) -> bool {
        let now = unix_now_ms();
        if !preedit_empty {
            self.session.postpone(now);
            self.arm(now, tick);
            return false;
        }
        if self.session.autosave_due(now) {
            self.session.save(notes);
            return true;
        }
        self.arm(now, tick);
        false
    }

    /// 立即保存没有写盘的修改。没有未保存的修改，或保存成功，返回真。
    pub(crate) fn flush(&mut self, notes: &NoteCommands) -> bool {
        self.timer.stop();
        if self.session.problem() == Some(&SaveProblem::Conflict) {
            return false;
        }
        match self.session.save(notes) {
            SaveOutcome::Clean | SaveOutcome::Saved => true,
            SaveOutcome::Problem => false,
        }
    }

    /// 没有未保存的修改，也没有等着用户选择的问题。
    pub(crate) fn resolved(&self) -> bool {
        !self.session.is_dirty() && self.session.problem().is_none()
    }

    pub(crate) fn problem_view(&self) -> ProblemView {
        let (kind, text) = match self.session.problem() {
            None => (0, String::new()),
            Some(SaveProblem::Failed(message)) => (1, message.clone()),
            Some(SaveProblem::Conflict) => (2, String::new()),
        };
        ProblemView {
            kind,
            text: text.into(),
            closing: self.after.is_some(),
        }
    }
}
