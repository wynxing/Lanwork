//! 置前时用的无按键鼠标移动，以及要等多久。不调用 Win32。
//!
//! `SetForegroundWindow` 允许「收到最后一次输入」的进程调用。`SendInput` 返回时，
//! 事件只是进了输入流，原始输入线程还没取走，立刻置前会和这一步竞争。
//! 这里只决定移动哪一种、以及等待的步进。真正的注入在 `crates/app`。

/// 注入之后至少等这么久，原始输入线程才来得及取出。
pub const NUDGE_MIN_WAIT_MS: u64 = 15;
/// 等不到新的最后输入时刻就停。界面线程在这期间不抽消息。
pub const NUDGE_MAX_WAIT_MS: u64 = 100;

/// 虚拟屏幕，物理像素。对应 `SM_XVIRTUALSCREEN` 和虚拟屏幕的宽高。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualScreen {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// 一次不会变成按键的鼠标移动。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZeroMove {
    /// 相对位移 0。光标不动。系统可能把这种空位移丢掉。
    Relative,
    /// 绝对坐标，归一化到 0..=65535。回投后仍是调用时的那一个像素。
    Absolute { x: i32, y: i32 },
}

/// 一次等待步进。调用方睡完再问下一次，不要在睡的时候抽消息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NudgeWait {
    /// 继续睡。最短等待没到时，这一段补到最短等待。
    Sleep {
        ms: u64,
    },
    /// 最后输入时刻已经比发送前新。再睡这一小段，让紧跟在后面的本次注入也被取走。
    Settle {
        ms: u64,
    },
    Done,
}

/// 选一次光标不动的移动。
///
/// 绝对坐标用 `(像素差 * 65535) / (边长 - 1)` 归一化，回投
/// `原点 + (归一化值 * (边长 - 1)) / 65535`。只有回投仍是同一像素时才用绝对坐标。
/// 对不上时用相对 0，避免按一套对不上的公式把光标挪开。
pub fn zero_move(cursor_x: i32, cursor_y: i32, screen: VirtualScreen) -> ZeroMove {
    match (
        absolute_axis(cursor_x, screen.x, screen.width),
        absolute_axis(cursor_y, screen.y, screen.height),
    ) {
        (Some(x), Some(y)) => ZeroMove::Absolute { x, y },
        _ => ZeroMove::Relative,
    }
}

/// 用和 [`zero_move`] 相同的除法把归一化坐标投回像素。
pub fn absolute_to_pixel(norm: i32, origin: i32, span: i32) -> Option<i32> {
    if span <= 1 || !(0..=65535).contains(&norm) {
        return None;
    }
    let back = i64::from(norm) * i64::from(span - 1) / 65535;
    i32::try_from(i64::from(origin) + back).ok()
}

/// `current` 是否严格晚于 `previous`。`GetTickCount` 约 49.7 天回绕，
/// 差值落在 `1..0x8000_0000` 里算更晚。
pub fn tick_after(previous: u32, current: u32) -> bool {
    let delta = current.wrapping_sub(previous);
    delta > 0 && delta < 0x8000_0000
}

/// 发送之后怎么等。`elapsed_ms` 从 `SendInput` 返回时算起。
/// `tick_advanced` 表示 `GetLastInputInfo` 已经比发送前新。
/// 最短等待没到时不看时刻，避免把发送前已经排着的用户输入当成自己的。
pub fn nudge_wait(elapsed_ms: u64, tick_advanced: bool, already_settled: bool) -> NudgeWait {
    if elapsed_ms < NUDGE_MIN_WAIT_MS {
        return NudgeWait::Sleep {
            ms: NUDGE_MIN_WAIT_MS - elapsed_ms,
        };
    }
    if elapsed_ms >= NUDGE_MAX_WAIT_MS {
        return NudgeWait::Done;
    }
    if tick_advanced {
        if already_settled {
            return NudgeWait::Done;
        }
        let settle = NUDGE_MIN_WAIT_MS.min(NUDGE_MAX_WAIT_MS - elapsed_ms);
        if settle == 0 {
            return NudgeWait::Done;
        }
        return NudgeWait::Settle { ms: settle };
    }
    NudgeWait::Sleep {
        ms: 5.min(NUDGE_MAX_WAIT_MS - elapsed_ms),
    }
}

fn absolute_axis(pixel: i32, origin: i32, span: i32) -> Option<i32> {
    if span <= 1 {
        return None;
    }
    let delta = i64::from(pixel) - i64::from(origin);
    if delta < 0 || delta >= i64::from(span) {
        return None;
    }
    let span_m1 = i64::from(span - 1);
    let numer = delta * 65535;
    let mut norm = numer / span_m1;
    if numer % span_m1 != 0 {
        norm += 1;
    }
    if !(0..=65535).contains(&norm) {
        return None;
    }
    let back = norm * span_m1 / 65535;
    (back == delta).then_some(i32::try_from(norm).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stays(cursor_x: i32, cursor_y: i32, screen: VirtualScreen) {
        match zero_move(cursor_x, cursor_y, screen) {
            ZeroMove::Relative => {}
            ZeroMove::Absolute { x, y } => {
                assert_eq!(
                    absolute_to_pixel(x, screen.x, screen.width),
                    Some(cursor_x),
                    "x {cursor_x} norm {x}"
                );
                assert_eq!(
                    absolute_to_pixel(y, screen.y, screen.height),
                    Some(cursor_y),
                    "y {cursor_y} norm {y}"
                );
            }
        }
    }

    #[test]
    fn primary_edges_and_axes_do_not_move() {
        let screen = VirtualScreen {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        for x in 0..screen.width {
            stays(x, 0, screen);
            stays(x, screen.height - 1, screen);
        }
        for y in 0..screen.height {
            stays(0, y, screen);
            stays(screen.width - 1, y, screen);
        }
    }

    #[test]
    fn monitor_left_of_primary_stays_put() {
        let screen = VirtualScreen {
            x: -1920,
            y: 0,
            width: 3840,
            height: 1080,
        };
        stays(-1920, 0, screen);
        stays(-1, 10, screen);
        stays(0, 10, screen);
        stays(1919, 1079, screen);
    }

    #[test]
    fn point_outside_the_virtual_screen_is_relative() {
        let screen = VirtualScreen {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(zero_move(-1, 0, screen), ZeroMove::Relative);
        assert_eq!(zero_move(1920, 0, screen), ZeroMove::Relative);
        assert_eq!(zero_move(0, 1080, screen), ZeroMove::Relative);
    }

    #[test]
    fn degenerate_screen_is_relative() {
        let screen = VirtualScreen {
            x: 0,
            y: 0,
            width: 1,
            height: 1080,
        };
        assert_eq!(zero_move(0, 10, screen), ZeroMove::Relative);
    }

    #[test]
    fn tick_after_handles_wrap() {
        assert!(tick_after(0, 1));
        assert!(!tick_after(0, 0));
        assert!(!tick_after(10, 10));
        assert!(tick_after(u32::MAX, 0));
        assert!(tick_after(0, 0x7fff_ffff));
        assert!(!tick_after(0, 0x8000_0000));
        assert!(!tick_after(100, 50));
    }

    #[test]
    fn wait_holds_the_minimum_then_settles_once() {
        assert_eq!(
            nudge_wait(0, true, false),
            NudgeWait::Sleep {
                ms: NUDGE_MIN_WAIT_MS
            }
        );
        assert_eq!(nudge_wait(14, false, false), NudgeWait::Sleep { ms: 1 });
        assert_eq!(
            nudge_wait(NUDGE_MIN_WAIT_MS, true, false),
            NudgeWait::Settle {
                ms: NUDGE_MIN_WAIT_MS
            }
        );
        assert_eq!(
            nudge_wait(NUDGE_MIN_WAIT_MS + NUDGE_MIN_WAIT_MS, true, true),
            NudgeWait::Done
        );
        assert_eq!(
            nudge_wait(NUDGE_MIN_WAIT_MS, false, false),
            NudgeWait::Sleep { ms: 5 }
        );
        assert_eq!(nudge_wait(98, false, false), NudgeWait::Sleep { ms: 2 });
        assert_eq!(nudge_wait(NUDGE_MAX_WAIT_MS, true, false), NudgeWait::Done);
        assert_eq!(nudge_wait(90, true, false), NudgeWait::Settle { ms: 10 });
    }
}
