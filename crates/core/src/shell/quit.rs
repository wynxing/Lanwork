//! 退出时把「界面正忙」和「便签没写盘」分开。
//!
//! 正忙是面板或便签悬浮窗的表借不出来，例如拖动、改大小、拖放或对话框的嵌套循环。
//! 这不是没写盘：约 100ms 后再试。大约 2 秒仍退不出就提示，不丢正文，也不静默结束。
//! 真正保存失败仍留下，由窗口上的「重试」和「放弃修改」接着走。

use std::cell::RefCell;
use std::time::Duration;

/// 借用冲突后再试的间隔。
pub const QUIT_RETRY_DELAY: Duration = Duration::from_millis(100);
/// 一直正忙超过这段时间，写警告并显示提示。之后仍重试，不丢正文。
pub const QUIT_BUSY_LIMIT: Duration = Duration::from_secs(2);

const REASON_BUSY: &str = "界面正忙";
const REASON_UNSAVED: &str = "便签没保存好";
const REASON_BOTH: &str = "便签没保存好，界面正忙";

/// 这次退出是谁提出的。写进日志，方便对照 `taskkill`、托盘和注销。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitSource {
    /// 事件循环隐藏目标窗口上的 `WM_CLOSE`（含不带 `/F` 的 `taskkill`）。
    WmClose,
    /// 托盘菜单「退出」。
    Tray,
    /// `WM_QUERYENDSESSION` 或 `WM_ENDSESSION`（`wParam` 非 0）。
    SessionEnd,
    /// 热键窗口上的 `WM_CLOSE`。
    HotkeyWindow,
    /// 已经核对过的安装包要退出后交给助手。
    Update,
}

impl QuitSource {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::WmClose => "WM_CLOSE",
            Self::Tray => "托盘",
            Self::SessionEnd => "会话结束",
            Self::HotkeyWindow => "热键窗口",
            Self::Update => "更新",
        }
    }
}

/// 一次保存尝试的结果。`busy` 是借不出来的表的个数，不算进 `unsaved`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct QuitFlush {
    pub unsaved: usize,
    pub busy: usize,
}

impl QuitFlush {
    #[must_use]
    pub const fn ready(unsaved: usize) -> Self {
        Self { unsaved, busy: 0 }
    }

    #[must_use]
    pub const fn busy() -> Self {
        Self {
            unsaved: 0,
            busy: 1,
        }
    }

    #[must_use]
    pub const fn merge(self, other: Self) -> Self {
        Self {
            unsaved: self.unsaved.saturating_add(other.unsaved),
            busy: self.busy.saturating_add(other.busy),
        }
    }
}

/// 和面板、便签悬浮窗退出时一样：借得到就保存，借不到记正忙，不把正忙当成没写盘。
#[must_use]
pub fn flush_while_borrowed<T>(
    cell: &RefCell<T>,
    flush: impl FnOnce(&mut T) -> usize,
) -> QuitFlush {
    match cell.try_borrow_mut() {
        Ok(mut value) => QuitFlush::ready(flush(&mut value)),
        Err(_) => QuitFlush::busy(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitLogLevel {
    Info,
    Warn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuitLog {
    pub level: QuitLogLevel,
    pub message: String,
}

impl QuitLog {
    fn info(message: String) -> Self {
        Self {
            level: QuitLogLevel::Info,
            message,
        }
    }

    fn warn(message: String) -> Self {
        Self {
            level: QuitLogLevel::Warn,
            message,
        }
    }
}

/// 这一轮尝试之后外壳要做的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuitEffect {
    Exit,
    /// 保存失败或冲突。窗口上已经有错误。不按计时器重试，也不丢正文。
    Stay {
        reason: &'static str,
    },
    /// 至少有一张表正忙。`delay` 之后再试。`notice` 超过约 2 秒时给出，只给一次。
    Retry {
        delay: Duration,
        notice: Option<String>,
    },
}

/// 同一次退出请求的来源和计时。后来的请求不把等待时间清零。
#[derive(Debug, Clone)]
pub struct QuitTrack {
    source: Option<QuitSource>,
    origin: Option<Duration>,
    noticed: bool,
}

impl Default for QuitTrack {
    fn default() -> Self {
        Self::new()
    }
}

impl QuitTrack {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            source: None,
            origin: None,
            noticed: false,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    #[must_use]
    pub fn source(&self) -> QuitSource {
        self.source.unwrap_or(QuitSource::WmClose)
    }

    /// 记下请求。第一次记下开始时间；之后只追加日志，不重新计时。
    #[must_use]
    pub fn request(&mut self, source: QuitSource, now: Duration) -> QuitLog {
        if self.origin.is_none() {
            self.origin = Some(now);
            self.source = Some(source);
        }
        request_log(source)
    }

    #[must_use]
    pub fn elapsed(&self, now: Duration) -> Duration {
        match self.origin {
            Some(origin) => now.saturating_sub(origin),
            None => Duration::ZERO,
        }
    }

    /// 按这一次的保存结果决定退出、留下，或稍后重试。
    #[must_use]
    pub fn advance(&mut self, flush: QuitFlush, now: Duration) -> (Vec<QuitLog>, QuitEffect) {
        let source = self.source();
        let elapsed = self.elapsed(now);
        if flush.unsaved == 0 && flush.busy == 0 {
            return (
                vec![outcome_log(source, flush, "退出", None, QuitLogLevel::Info)],
                QuitEffect::Exit,
            );
        }
        if flush.busy == 0 {
            return (
                vec![outcome_log(
                    source,
                    flush,
                    "留下",
                    Some(REASON_UNSAVED),
                    QuitLogLevel::Info,
                )],
                QuitEffect::Stay {
                    reason: REASON_UNSAVED,
                },
            );
        }
        let reason = if flush.unsaved > 0 {
            REASON_BOTH
        } else {
            REASON_BUSY
        };
        let mut logs = vec![outcome_log(
            source,
            flush,
            "稍后重试",
            Some(reason),
            QuitLogLevel::Info,
        )];
        let notice = if elapsed >= QUIT_BUSY_LIMIT && !self.noticed {
            self.noticed = true;
            logs.push(QuitLog::warn(format!(
                "退出仍未完成：来源={}，未保存={}，借用冲突={}，原因={reason}",
                source.label(),
                flush.unsaved,
                flush.busy,
            )));
            Some(blocked_notice(reason))
        } else {
            None
        };
        (
            logs,
            QuitEffect::Retry {
                delay: QUIT_RETRY_DELAY,
                notice,
            },
        )
    }
}

#[must_use]
pub fn request_log(source: QuitSource) -> QuitLog {
    QuitLog::info(format!("退出请求：来源={}", source.label()))
}

#[must_use]
pub fn already_exiting_log(source: QuitSource) -> QuitLog {
    QuitLog::info(format!(
        "退出结果：来源={}，未保存=0，借用冲突=0，决定=已在退出",
        source.label()
    ))
}

/// `attempt_quit` 重入：上一次尝试还没返回。当成正忙，稍后重试，不静默丢掉。
#[must_use]
pub fn reentered_log(source: QuitSource) -> QuitLog {
    outcome_log(
        source,
        QuitFlush::busy(),
        "稍后重试",
        Some(REASON_BUSY),
        QuitLogLevel::Info,
    )
}

#[must_use]
pub fn blocked_notice(reason: &str) -> String {
    format!("还有便签没保存好，暂时无法退出：{reason}")
}

fn outcome_log(
    source: QuitSource,
    flush: QuitFlush,
    decision: &str,
    reason: Option<&str>,
    level: QuitLogLevel,
) -> QuitLog {
    let mut message = format!(
        "退出结果：来源={}，未保存={}，借用冲突={}，决定={decision}",
        source.label(),
        flush.unsaved,
        flush.busy,
    );
    if let Some(reason) = reason {
        message.push_str("，原因=");
        message.push_str(reason);
    }
    QuitLog { level, message }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::time::Duration;

    use super::*;

    fn drive(
        track: &mut QuitTrack,
        panel: &RefCell<u8>,
        notes: &RefCell<u8>,
        now: Duration,
    ) -> (Vec<QuitLog>, QuitEffect) {
        let flush = flush_while_borrowed(panel, |_| 0).merge(flush_while_borrowed(notes, |_| 0));
        track.advance(flush, now)
    }

    #[test]
    fn request_quit_while_panel_and_note_state_are_borrowed_retries_then_exits() {
        let panel = RefCell::new(0_u8);
        let notes = RefCell::new(0_u8);
        let mut track = QuitTrack::new();
        let mut logs = vec![track.request(QuitSource::WmClose, Duration::ZERO)];
        let panel_hold = panel.borrow();
        let note_hold = notes.borrow();

        let (more, effect) = drive(&mut track, &panel, &notes, Duration::ZERO);
        logs.extend(more);
        let mut next = match effect {
            QuitEffect::Retry { delay, notice } => {
                assert!(notice.is_none(), "刚开始正忙先重试，不到 2 秒不提示");
                delay
            }
            other => panic!("借用冲突应当稍后重试，得到 {other:?}"),
        };
        assert!(
            logs.iter().any(|line| {
                line.level == QuitLogLevel::Info
                    && line.message.contains("未保存=0")
                    && line.message.contains("借用冲突=2")
                    && line.message.contains("决定=稍后重试")
                    && line.message.contains("原因=界面正忙")
            }),
            "正忙不能记成没保存：{logs:?}"
        );

        let mut now = Duration::ZERO;
        let mut exited = false;
        let mut stayed = false;
        while now < Duration::from_secs(3) {
            now = next;
            let (more, effect) = drive(&mut track, &panel, &notes, now);
            logs.extend(more);
            match effect {
                QuitEffect::Retry { delay, .. } => next = now + delay,
                QuitEffect::Exit => {
                    exited = true;
                    break;
                }
                QuitEffect::Stay { .. } => {
                    stayed = true;
                    break;
                }
            }
        }
        assert!(!exited, "表还借着不能退出");
        assert!(!stayed, "借用冲突不能当成没保存而一直留下");
        assert_eq!(
            logs.iter()
                .filter(|line| line.level == QuitLogLevel::Warn)
                .count(),
            1,
            "超过约 2 秒只警告一次：{logs:?}"
        );
        assert!(
            logs.iter().any(|line| {
                line.level == QuitLogLevel::Warn && line.message.contains("退出仍未完成")
            }),
            "{logs:?}"
        );

        drop(panel_hold);
        drop(note_hold);
        let (more, effect) = drive(&mut track, &panel, &notes, next);
        logs.extend(more);
        assert!(
            matches!(effect, QuitEffect::Exit),
            "松开之后应当退出，得到 {effect:?}"
        );
        assert!(
            logs.iter().any(|line| line.message.contains("决定=退出")),
            "{logs:?}"
        );
    }

    #[test]
    fn each_refusal_path_writes_a_log_line() {
        let mut track = QuitTrack::new();
        let request = track.request(QuitSource::Tray, Duration::ZERO);
        assert_eq!(request.level, QuitLogLevel::Info);
        assert!(request.message.contains("退出请求：来源=托盘"));

        let (logs, effect) = track.advance(QuitFlush::busy(), Duration::from_millis(100));
        assert!(matches!(effect, QuitEffect::Retry { notice: None, .. }));
        assert!(
            logs.iter().any(|line| {
                line.level == QuitLogLevel::Info
                    && line.message.contains("来源=托盘")
                    && line.message.contains("未保存=0")
                    && line.message.contains("借用冲突=1")
                    && line.message.contains("决定=稍后重试")
                    && line.message.contains("原因=界面正忙")
            }),
            "{logs:?}"
        );

        let (logs, effect) = track.advance(QuitFlush::ready(2), Duration::ZERO);
        assert!(matches!(
            effect,
            QuitEffect::Stay {
                reason: REASON_UNSAVED,
            }
        ));
        assert!(
            logs.iter().any(|line| {
                line.level == QuitLogLevel::Info
                    && line.message.contains("未保存=2")
                    && line.message.contains("借用冲突=0")
                    && line.message.contains("决定=留下")
                    && line.message.contains("原因=便签没保存好")
            }),
            "{logs:?}"
        );

        let (logs, effect) = track.advance(QuitFlush::busy(), QUIT_BUSY_LIMIT);
        match effect {
            QuitEffect::Retry {
                notice: Some(text), ..
            } => {
                assert_eq!(text, "还有便签没保存好，暂时无法退出：界面正忙");
            }
            other => panic!("超时仍正忙应当提示并继续重试，得到 {other:?}"),
        }
        assert!(
            logs.iter().any(|line| {
                line.level == QuitLogLevel::Warn
                    && line.message.contains("退出仍未完成")
                    && line.message.contains("借用冲突=1")
                    && line.message.contains("原因=界面正忙")
            }),
            "{logs:?}"
        );

        let reentered = reentered_log(QuitSource::HotkeyWindow);
        assert_eq!(reentered.level, QuitLogLevel::Info);
        assert!(reentered.message.contains("来源=热键窗口"));
        assert!(reentered.message.contains("决定=稍后重试"));
        assert!(reentered.message.contains("原因=界面正忙"));

        let session = request_log(QuitSource::SessionEnd);
        assert!(session.message.contains("退出请求：来源=会话结束"));
        let exiting = already_exiting_log(QuitSource::WmClose);
        assert!(exiting.message.contains("决定=已在退出"));
    }

    #[test]
    fn save_failure_stays_and_surfaces_the_error() {
        let notes = RefCell::new(true);
        let surfaced = Cell::new(false);
        let flush = flush_while_borrowed(&notes, |failed| {
            if *failed {
                // 这里代替把便签窗口提到前面并写上保存错误。
                surfaced.set(true);
                1
            } else {
                0
            }
        });
        let mut track = QuitTrack::new();
        let request = track.request(QuitSource::HotkeyWindow, Duration::ZERO);
        assert!(request.message.contains("来源=热键窗口"));
        let (logs, effect) = track.advance(flush, QUIT_BUSY_LIMIT);
        assert!(surfaced.get(), "保存失败要把错误露出来");
        assert!(matches!(
            effect,
            QuitEffect::Stay {
                reason: REASON_UNSAVED,
            }
        ));
        assert!(
            logs.iter().any(|line| {
                line.level == QuitLogLevel::Info
                    && line.message.contains("决定=留下")
                    && line.message.contains("未保存=1")
                    && line.message.contains("原因=便签没保存好")
            }),
            "{logs:?}"
        );
        assert!(
            logs.iter().all(|line| line.level == QuitLogLevel::Info),
            "保存失败不是超时警告：{logs:?}"
        );

        let (logs, effect) = track.advance(QuitFlush::ready(0), QUIT_BUSY_LIMIT);
        assert!(
            matches!(effect, QuitEffect::Exit),
            "放弃或保存成功之后才退出，得到 {effect:?}"
        );
        assert!(logs.iter().any(|line| line.message.contains("决定=退出")));
    }

    #[test]
    fn save_failure_still_surfaces_when_the_other_table_is_busy() {
        let panel = RefCell::new(());
        let notes = RefCell::new(());
        let surfaced = Cell::new(false);
        let hold = panel.borrow();
        let flush = flush_while_borrowed(&panel, |_| 0).merge(flush_while_borrowed(&notes, |_| {
            surfaced.set(true);
            1
        }));
        drop(hold);
        assert!(surfaced.get());
        assert_eq!(
            flush,
            QuitFlush {
                unsaved: 1,
                busy: 1,
            }
        );
        let mut track = QuitTrack::new();
        let _ = track.request(QuitSource::SessionEnd, Duration::ZERO);
        let (logs, effect) = track.advance(flush, Duration::from_millis(100));
        assert!(
            matches!(effect, QuitEffect::Retry { notice: None, .. }),
            "一边没保存、一边正忙时不能退出，也不能停在不再重试，得到 {effect:?}"
        );
        assert!(
            logs.iter().any(|line| {
                line.message.contains("未保存=1")
                    && line.message.contains("借用冲突=1")
                    && line.message.contains("决定=稍后重试")
                    && line.message.contains("来源=会话结束")
            }),
            "{logs:?}"
        );
    }

    #[test]
    fn later_quit_request_does_not_reset_the_busy_wait() {
        let mut track = QuitTrack::new();
        let first = track.request(QuitSource::WmClose, Duration::ZERO);
        let second = track.request(QuitSource::Tray, Duration::from_secs(5));
        assert!(first.message.contains("来源=WM_CLOSE"));
        assert!(second.message.contains("来源=托盘"));
        let (logs, effect) = track.advance(QuitFlush::busy(), Duration::from_secs(5));
        match effect {
            QuitEffect::Retry {
                notice: Some(text), ..
            } => assert_eq!(text, blocked_notice(REASON_BUSY)),
            other => panic!("第二次请求不能把 2 秒计时清零，得到 {other:?}"),
        }
        assert!(
            logs.iter()
                .any(|line| line.message.contains("来源=WM_CLOSE")),
            "结果沿用第一次请求的来源：{logs:?}"
        );
    }
}
