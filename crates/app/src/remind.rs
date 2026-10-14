//! 到期提醒调度。按下一次提醒或安静时段结束设一个定时器，不按秒轮询。
//!
//! 睡眠唤醒和系统时间变化也重新计算。通知发不出去时只记日志，角标照常更新。
//! 安静时段来自配置里的 `quietHours`。配置变更会重新计算。

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::time::Duration;

use lanwork_core::CivilDate;
use lanwork_core::config::ConfigCommands;
use lanwork_core::storage::Store;
use lanwork_core::todos::{
    ClockTime, QuietSpan, ReminderInstant, SentKey, TodoCommands, wait_millis,
};
use windows::Win32::System::SystemInformation::GetLocalTime;

struct State {
    todos: TodoCommands,
    config: ConfigCommands,
    store: Store,
    /// 这次进程里发出失败的提醒。外部事件（待办变更、配置变更、唤醒、时间变化）会清掉再试。
    failed: BTreeSet<SentKey>,
}

struct LocalClock {
    now: ReminderInstant,
    second: u32,
    millis: u32,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static TIMER: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
}

pub(crate) fn install(todos: TodoCommands, config: ConfigCommands, store: Store, installed: bool) {
    crate::toast::prepare(&store, installed);
    STATE.with(|slot| {
        *slot.borrow_mut() = Some(State {
            todos,
            config,
            store,
            failed: BTreeSet::new(),
        });
    });
    slint::Timer::single_shot(Duration::ZERO, move || {
        if installed {
            crate::platform::request_resume_activator();
        }
        on_external();
    });
}

/// 待办或配置变了、系统从睡眠唤醒、或系统时间变了。
pub(crate) fn on_external() {
    evaluate(true);
}

fn evaluate(clear_failed: bool) {
    if let Some(err) = crate::toast::take_runtime_error() {
        log_warn(&err);
    }
    crate::toast::flush_pending();
    let Some(clock) = local_clock() else {
        log_warn("读不到本地时间，到期提醒没有调度");
        return;
    };
    let plan = STATE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let state = slot.as_mut()?;
        if clear_failed {
            state.failed.clear();
        }
        let plan = match state
            .todos
            .reminder_plan(clock.now, quiet_span(&state.config))
        {
            Ok(plan) => plan,
            Err(err) => {
                state.store.log_warn(&format!("到期提醒没有计算：{err}"));
                return None;
            }
        };
        crate::host::refresh_overdue(&state.todos, &state.store);
        crate::todo_float::on_todos_changed();
        let mut shown = Vec::new();
        for item in &plan.due_now {
            let key = SentKey::from_due(item);
            if state.failed.contains(&key) {
                continue;
            }
            match crate::toast::show_one(item) {
                Ok(()) => shown.push(key),
                Err(err) => {
                    state.store.log_warn(&format!("到期通知没有发出：{err}"));
                    state.failed.insert(key);
                }
            }
        }
        let retry_missed = plan
            .missed
            .iter()
            .any(|item| !state.failed.contains(&SentKey::from_due(item)));
        if retry_missed {
            match crate::toast::show_missed(&plan.missed) {
                Ok(()) => {
                    for item in &plan.missed {
                        shown.push(SentKey::from_due(item));
                    }
                }
                Err(err) => {
                    state.store.log_warn(&format!("到期通知没有发出：{err}"));
                    for item in &plan.missed {
                        state.failed.insert(SentKey::from_due(item));
                    }
                }
            }
        }
        if !shown.is_empty()
            && let Err(err) = state.todos.mark_reminders_sent(&shown)
        {
            state.store.log_warn(&format!("到期提醒没有记下：{err}"));
        }
        Some(plan.next)
    });
    let Some(next) = plan else {
        return;
    };
    arm(next, &clock);
}

fn arm(next: Option<ReminderInstant>, clock: &LocalClock) {
    let Some(next) = next else {
        TIMER.with(|slot| slot.borrow_mut().take());
        return;
    };
    let ms = wait_millis(clock.now, next, clock.second, clock.millis);
    if ms == 0 {
        return;
    }
    TIMER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let timer = slot.get_or_insert_with(slint::Timer::default);
        timer.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(ms),
            || {
                evaluate(false);
            },
        );
    });
}

fn quiet_span(config: &ConfigCommands) -> Option<QuietSpan> {
    config
        .current()
        .quiet_hours
        .map(|hours| QuietSpan::from_clocks(hours.start, hours.end))
}

fn log_warn(message: &str) {
    STATE.with(|slot| {
        if let Some(state) = slot.borrow().as_ref() {
            state.store.log_warn(message);
        }
    });
}

fn local_clock() -> Option<LocalClock> {
    // SAFETY: GetLocalTime 只读系统时钟，没有输出缓冲区。
    let time = unsafe { GetLocalTime() };
    let date = CivilDate::try_from_ymd(
        i32::from(time.wYear),
        u8::try_from(time.wMonth).ok()?,
        u8::try_from(time.wDay).ok()?,
    )?;
    let clock = ClockTime::try_new(
        u8::try_from(time.wHour).ok()?,
        u8::try_from(time.wMinute).ok()?,
    )?;
    Some(LocalClock {
        now: ReminderInstant { date, time: clock },
        second: u32::from(time.wSecond),
        millis: u32::from(time.wMilliseconds),
    })
}
