//! 设置页录入热键：把一次按键（虚拟键码和修饰键）换成 [`Hotkey`]。
//!
//! 不调用 Win32。按键从哪里来、录入期间怎样让已注册的全局热键不抢键，在 `crates/app`。
//! 能录入的键和修饰键规则与 [`crate::config::parse_hotkey`] 相同：除 `F1`–`F24` 外必须
//! 至少有一个修饰键，功能键可以单独使用。
//!
//! NumLock 关掉时，小键盘数字的虚拟键码和导航键相同，只有扩展位不同。扩展位为 0 的记成
//! 对应的 `Numpad0`–`Numpad9`；扩展位为 1 的才是独立导航键。不读 NumLock。小键盘小数点是
//! 非扩展的 Delete，不在键名表里，拒绝。

use crate::config::{Hotkey, HotkeyKey};

const VK_ESCAPE: u32 = 0x1B;
const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
const VK_MENU: u32 = 0x12;
const VK_LWIN: u32 = 0x5B;
const VK_RWIN: u32 = 0x5C;
const VK_LSHIFT: u32 = 0xA0;
const VK_RMENU: u32 = 0xA5;
/// 输入法正在处理这次按键，真正的键码拿不到。
const VK_PROCESSKEY: u32 = 0xE5;
const VK_CLEAR: u32 = 0x0C;
const VK_PRIOR: u32 = 0x21;
const VK_NEXT: u32 = 0x22;
const VK_END: u32 = 0x23;
const VK_HOME: u32 = 0x24;
const VK_LEFT: u32 = 0x25;
const VK_UP: u32 = 0x26;
const VK_RIGHT: u32 = 0x27;
const VK_DOWN: u32 = 0x28;
const VK_INSERT: u32 = 0x2D;
const VK_DELETE: u32 = 0x2E;

/// 按下时的修饰键状态。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyMods {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
}

impl KeyMods {
    fn any(self) -> bool {
        self.control || self.alt || self.shift || self.win
    }
}

/// 一次按键的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureStep {
    /// 只按下了修饰键，或输入法吞了这次按键。继续等。
    Waiting,
    /// 没有修饰键的 Esc。放弃录入。
    Cancelled,
    Captured(Hotkey),
    /// 不能作为热键的键：不在键名表里，或除功能键外没带修饰键。
    Rejected,
}

fn is_modifier_key(vk: u32) -> bool {
    matches!(vk, VK_SHIFT | VK_CONTROL | VK_MENU | VK_LWIN | VK_RWIN)
        || (VK_LSHIFT..=VK_RMENU).contains(&vk)
}

/// 非扩展的导航键来自小键盘。小数点不在键名表里。
enum NumpadAlias {
    Digit(HotkeyKey),
    Decimal,
}

/// `extended` 为假且 `vk` 是导航键或 `VK_CLEAR` 时，这是小键盘数字。
fn numpad_alias(vk: u32) -> Option<NumpadAlias> {
    Some(match vk {
        VK_INSERT => NumpadAlias::Digit(HotkeyKey::Numpad(0)),
        VK_END => NumpadAlias::Digit(HotkeyKey::Numpad(1)),
        VK_DOWN => NumpadAlias::Digit(HotkeyKey::Numpad(2)),
        VK_NEXT => NumpadAlias::Digit(HotkeyKey::Numpad(3)),
        VK_LEFT => NumpadAlias::Digit(HotkeyKey::Numpad(4)),
        VK_CLEAR => NumpadAlias::Digit(HotkeyKey::Numpad(5)),
        VK_RIGHT => NumpadAlias::Digit(HotkeyKey::Numpad(6)),
        VK_HOME => NumpadAlias::Digit(HotkeyKey::Numpad(7)),
        VK_UP => NumpadAlias::Digit(HotkeyKey::Numpad(8)),
        VK_PRIOR => NumpadAlias::Digit(HotkeyKey::Numpad(9)),
        VK_DELETE => NumpadAlias::Decimal,
        _ => return None,
    })
}

/// 把一次按下换成录入结果。`vk` 是 Win32 虚拟键码。`extended` 是 `WM_KEYDOWN` 的扩展位。
#[must_use]
pub fn capture_step(vk: u32, mods: KeyMods, extended: bool) -> CaptureStep {
    if is_modifier_key(vk) || vk == VK_PROCESSKEY {
        return CaptureStep::Waiting;
    }
    if vk == VK_ESCAPE && !mods.any() {
        return CaptureStep::Cancelled;
    }
    let key = if extended {
        HotkeyKey::from_virtual_key(vk)
    } else {
        match numpad_alias(vk) {
            Some(NumpadAlias::Digit(key)) => Some(key),
            Some(NumpadAlias::Decimal) => return CaptureStep::Rejected,
            None => HotkeyKey::from_virtual_key(vk),
        }
    };
    let Some(key) = key else {
        return CaptureStep::Rejected;
    };
    if key.requires_modifier() && !mods.any() {
        return CaptureStep::Rejected;
    }
    CaptureStep::Captured(Hotkey {
        control: mods.control,
        alt: mods.alt,
        shift: mods.shift,
        win: mods.win,
        key,
    })
}

/// 录入结束的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    Cancelled,
    Captured(Hotkey),
    Rejected,
}

/// 录入状态。只记「正在录入」这一位，不保存半截的组合。
#[derive(Debug, Default)]
pub struct HotkeyRecorder {
    recording: bool,
}

impl HotkeyRecorder {
    #[must_use]
    pub const fn new() -> Self {
        Self { recording: false }
    }

    #[must_use]
    pub fn is_recording(&self) -> bool {
        self.recording
    }

    /// 开始录入。已经在录入时返回 false，什么也不改。
    pub fn start(&mut self) -> bool {
        if self.recording {
            return false;
        }
        self.recording = true;
        true
    }

    /// 放弃录入。没有在录入时什么也不做。
    pub fn stop(&mut self) {
        self.recording = false;
    }

    /// 喂一次按下。结束录入的按键返回结果并退出录入；只按了修饰键，或没有在录入，返回 `None`。
    /// 录入结束之后迟到的按键不会再产生结果。
    pub fn feed(&mut self, vk: u32, mods: KeyMods, extended: bool) -> Option<Recorded> {
        if !self.recording {
            return None;
        }
        let recorded = match capture_step(vk, mods, extended) {
            CaptureStep::Waiting => return None,
            CaptureStep::Cancelled => Recorded::Cancelled,
            CaptureStep::Captured(hotkey) => Recorded::Captured(hotkey),
            CaptureStep::Rejected => Recorded::Rejected,
        };
        self.recording = false;
        Some(recorded)
    }
}

/// 窗口收到的一条键盘消息，由外壳从 `WM_KEYDOWN`、`WM_KEYUP`、`WM_CHAR` 等换算而来。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEvent {
    Down {
        vk: u32,
        mods: KeyMods,
        repeat: bool,
        /// `WM_KEYDOWN` lParam 第 24 位。为 0 时，导航键虚拟键码来自小键盘。
        extended: bool,
    },
    Up {
        vk: u32,
    },
    /// `WM_CHAR` 一类由按键翻译出来的字符消息。
    Char,
}

/// 窗口过程对这条消息的处理。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// 交给下一层，照常处理。
    Pass,
    /// 吞掉，不让窗口和系统菜单看到。
    Swallow,
    /// 录入结束，这条消息也被吞掉。
    Recorded(Recorded),
}

/// 录入期间的按键过滤。
///
/// 录入时所有键盘消息都被吞掉，Alt 组合键不会激活窗口菜单，Esc 不会收起面板。
/// 结束录入的那个键还没松开时，它的自动重复、字符消息和抬起也继续吞掉，
/// 否则 Esc 取消录入后按住不放，面板会在重复的 Esc 上收起。
#[derive(Debug, Default)]
pub struct KeyGate {
    recorder: HotkeyRecorder,
    draining: Option<u32>,
}

impl KeyGate {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            recorder: HotkeyRecorder::new(),
            draining: None,
        }
    }

    #[must_use]
    pub fn is_recording(&self) -> bool {
        self.recorder.is_recording()
    }

    /// 开始录入。已经在录入时返回 false。
    pub fn begin(&mut self) -> bool {
        self.draining = None;
        self.recorder.start()
    }

    /// 放弃录入并忘掉等待抬起的键。窗口失焦、收起时调用。
    pub fn reset(&mut self) {
        self.recorder.stop();
        self.draining = None;
    }

    pub fn route(&mut self, event: KeyEvent) -> Route {
        if self.recorder.is_recording() {
            return match event {
                KeyEvent::Down {
                    vk, mods, extended, ..
                } => match self.recorder.feed(vk, mods, extended) {
                    Some(recorded) => {
                        self.draining = Some(vk);
                        Route::Recorded(recorded)
                    }
                    None => Route::Swallow,
                },
                KeyEvent::Up { .. } | KeyEvent::Char => Route::Swallow,
            };
        }
        let Some(held) = self.draining else {
            return Route::Pass;
        };
        match event {
            KeyEvent::Char => Route::Swallow,
            KeyEvent::Down { vk, repeat, .. } if vk == held => {
                if repeat {
                    Route::Swallow
                } else {
                    self.draining = None;
                    Route::Pass
                }
            }
            KeyEvent::Up { vk } if vk == held => {
                self.draining = None;
                Route::Swallow
            }
            _ => Route::Pass,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::parse_hotkey;

    const VK_F1: u32 = 0x70;
    const VK_F24: u32 = 0x87;

    const CTRL_ALT: KeyMods = KeyMods {
        control: true,
        alt: true,
        shift: false,
        win: false,
    };

    fn captured(vk: u32, mods: KeyMods) -> String {
        match capture_step(vk, mods, true) {
            CaptureStep::Captured(hotkey) => hotkey.canonical(),
            other => panic!("expected a hotkey, got {other:?}"),
        }
    }

    #[test]
    fn ctrl_alt_m_is_the_default_hotkey() {
        assert_eq!(captured(u32::from(b'M'), CTRL_ALT), "Ctrl+Alt+M");
        assert_eq!(
            captured(u32::from(b'M'), CTRL_ALT),
            crate::config::default_search_hotkey()
        );
    }

    #[test]
    fn modifier_order_is_fixed_whatever_was_pressed_first() {
        let all = KeyMods {
            control: true,
            alt: true,
            shift: true,
            win: true,
        };
        assert_eq!(captured(u32::from(b'7'), all), "Ctrl+Alt+Shift+Win+7");
        let shift_win = KeyMods {
            shift: true,
            win: true,
            ..KeyMods::default()
        };
        assert_eq!(
            captured(u32::from(b'k'.to_ascii_uppercase()), shift_win),
            "Shift+Win+K"
        );
    }

    #[test]
    fn every_captured_hotkey_is_accepted_by_the_config_parser() {
        let mods = [
            KeyMods::default(),
            CTRL_ALT,
            KeyMods {
                shift: true,
                ..KeyMods::default()
            },
        ];
        let keys = (0u32..=0xFF).filter(|vk| HotkeyKey::from_virtual_key(*vk).is_some());
        for vk in keys {
            for item in mods {
                if let CaptureStep::Captured(hotkey) = capture_step(vk, item, true) {
                    let parsed = parse_hotkey(&hotkey.canonical()).unwrap();
                    assert_eq!(parsed, hotkey);
                    assert_eq!(parsed.virtual_key(), vk);
                }
            }
        }
    }

    #[test]
    fn function_keys_work_alone_but_other_keys_need_a_modifier() {
        assert_eq!(captured(VK_F1, KeyMods::default()), "F1");
        assert_eq!(captured(VK_F24, KeyMods::default()), "F24");
        assert_eq!(
            capture_step(u32::from(b'M'), KeyMods::default(), true),
            CaptureStep::Rejected
        );
        assert_eq!(
            capture_step(u32::from(b'5'), KeyMods::default(), true),
            CaptureStep::Rejected
        );
        assert_eq!(
            capture_step(0x20, KeyMods::default(), true),
            CaptureStep::Rejected
        );
        assert_eq!(
            capture_step(VK_F24 + 1, KeyMods::default(), true),
            CaptureStep::Rejected
        );
    }

    #[test]
    fn widened_keys_are_captured_with_the_same_text_as_config() {
        let cases = [
            (
                0x20,
                KeyMods {
                    control: true,
                    ..KeyMods::default()
                },
                "Ctrl+Space",
            ),
            (
                0x25,
                KeyMods {
                    alt: true,
                    ..KeyMods::default()
                },
                "Alt+Left",
            ),
            (
                0xBF,
                KeyMods {
                    control: true,
                    shift: true,
                    ..KeyMods::default()
                },
                "Ctrl+Shift+/",
            ),
            (
                0x61,
                KeyMods {
                    control: true,
                    ..KeyMods::default()
                },
                "Ctrl+Numpad1",
            ),
        ];
        for (vk, mods, text) in cases {
            assert_eq!(captured(vk, mods), text);
            let parsed = parse_hotkey(text).unwrap();
            assert_eq!(parsed.virtual_key(), vk);
            assert_eq!(capture_step(vk, mods, true), CaptureStep::Captured(parsed));
        }
        assert_eq!(
            capture_step(0x20, KeyMods::default(), true),
            CaptureStep::Rejected
        );
        assert!(parse_hotkey("Space").is_err());
    }

    #[test]
    fn non_extended_navigation_keys_are_numpad_digits() {
        let ctrl = KeyMods {
            control: true,
            ..KeyMods::default()
        };
        let digits = [
            (VK_INSERT, "Ctrl+Numpad0"),
            (VK_END, "Ctrl+Numpad1"),
            (VK_DOWN, "Ctrl+Numpad2"),
            (VK_NEXT, "Ctrl+Numpad3"),
            (VK_LEFT, "Ctrl+Numpad4"),
            (VK_CLEAR, "Ctrl+Numpad5"),
            (VK_RIGHT, "Ctrl+Numpad6"),
            (VK_HOME, "Ctrl+Numpad7"),
            (VK_UP, "Ctrl+Numpad8"),
            (VK_PRIOR, "Ctrl+Numpad9"),
        ];
        for (vk, text) in digits {
            let parsed = parse_hotkey(text).unwrap();
            assert_eq!(
                capture_step(vk, ctrl, false),
                CaptureStep::Captured(parsed),
                "{text}"
            );
            if vk == VK_CLEAR {
                assert_eq!(capture_step(vk, ctrl, true), CaptureStep::Rejected);
            } else {
                let extended = match capture_step(vk, ctrl, true) {
                    CaptureStep::Captured(hotkey) => hotkey,
                    other => panic!("{text}: {other:?}"),
                };
                assert_eq!(extended.virtual_key(), vk, "{text}");
                assert_ne!(extended, parsed, "{text}");
            }
        }
        assert_eq!(capture_step(VK_DELETE, ctrl, false), CaptureStep::Rejected);
        assert_eq!(
            capture_step(VK_DELETE, ctrl, true),
            CaptureStep::Captured(parse_hotkey("Ctrl+Delete").unwrap())
        );
        assert_eq!(
            capture_step(0x64, ctrl, false),
            CaptureStep::Captured(parse_hotkey("Ctrl+Numpad4").unwrap())
        );

        let shifted = KeyMods {
            control: true,
            shift: true,
            ..KeyMods::default()
        };
        assert_eq!(
            capture_step(VK_LEFT, shifted, false),
            CaptureStep::Captured(parse_hotkey("Ctrl+Shift+Numpad4").unwrap())
        );
        assert_eq!(
            capture_step(VK_LEFT, shifted, true),
            CaptureStep::Captured(parse_hotkey("Ctrl+Shift+Left").unwrap())
        );
    }

    #[test]
    fn the_gate_passes_the_extended_bit_into_the_same_rule() {
        let ctrl = KeyMods {
            control: true,
            ..KeyMods::default()
        };
        let mut gate = KeyGate::new();
        gate.begin();
        assert_eq!(
            gate.route(down_ext(VK_LEFT, ctrl, false)),
            Route::Recorded(Recorded::Captured(parse_hotkey("Ctrl+Numpad4").unwrap()))
        );

        gate.begin();
        assert_eq!(
            gate.route(down_ext(VK_LEFT, ctrl, true)),
            Route::Recorded(Recorded::Captured(parse_hotkey("Ctrl+Left").unwrap()))
        );

        gate.begin();
        assert_eq!(
            gate.route(down_ext(VK_DELETE, ctrl, false)),
            Route::Recorded(Recorded::Rejected)
        );

        gate.begin();
        assert_eq!(
            gate.route(down_ext(VK_CLEAR, ctrl, false)),
            Route::Recorded(Recorded::Captured(parse_hotkey("Ctrl+Numpad5").unwrap()))
        );
    }

    #[test]
    fn keys_outside_the_table_are_rejected() {
        for vk in [0x0D, 0x09, 0x08, 0x6C, 0x6E, 0x2C] {
            assert_eq!(
                capture_step(vk, CTRL_ALT, true),
                CaptureStep::Rejected,
                "{vk:#x}"
            );
        }
    }

    #[test]
    fn modifier_keys_and_input_method_keys_keep_waiting() {
        for vk in [
            VK_SHIFT,
            VK_CONTROL,
            VK_MENU,
            VK_LWIN,
            VK_RWIN,
            VK_LSHIFT,
            0xA1,
            0xA2,
            0xA3,
            0xA4,
            VK_RMENU,
            VK_PROCESSKEY,
        ] {
            assert_eq!(
                capture_step(vk, CTRL_ALT, true),
                CaptureStep::Waiting,
                "{vk:#x}"
            );
        }
    }

    #[test]
    fn escape_cancels_only_without_a_modifier() {
        assert_eq!(
            capture_step(VK_ESCAPE, KeyMods::default(), true),
            CaptureStep::Cancelled
        );
        assert_eq!(
            capture_step(VK_ESCAPE, CTRL_ALT, true),
            CaptureStep::Rejected
        );
    }

    #[test]
    fn recorder_ends_on_the_first_decisive_key_and_ignores_later_ones() {
        let mut recorder = HotkeyRecorder::new();
        assert_eq!(recorder.feed(u32::from(b'M'), CTRL_ALT, false), None);
        assert!(recorder.start());
        assert!(!recorder.start());
        assert!(recorder.is_recording());
        assert_eq!(recorder.feed(VK_CONTROL, CTRL_ALT, false), None);
        assert_eq!(recorder.feed(VK_MENU, CTRL_ALT, false), None);
        assert!(recorder.is_recording());

        let done = recorder.feed(u32::from(b'N'), CTRL_ALT, false);
        assert_eq!(
            done,
            Some(Recorded::Captured(parse_hotkey("Ctrl+Alt+N").unwrap()))
        );
        assert!(!recorder.is_recording());
        assert_eq!(recorder.feed(u32::from(b'K'), CTRL_ALT, false), None);
    }

    #[test]
    fn recorder_reports_cancel_and_rejection_and_can_start_again() {
        let mut recorder = HotkeyRecorder::new();
        recorder.start();
        assert_eq!(
            recorder.feed(VK_ESCAPE, KeyMods::default(), false),
            Some(Recorded::Cancelled)
        );
        assert!(!recorder.is_recording());

        recorder.start();
        assert_eq!(
            recorder.feed(0x0D, CTRL_ALT, false),
            Some(Recorded::Rejected)
        );
        assert!(!recorder.is_recording());

        recorder.start();
        recorder.stop();
        assert_eq!(recorder.feed(u32::from(b'M'), CTRL_ALT, false), None);
        assert!(recorder.start());
    }

    fn down(vk: u32, mods: KeyMods) -> KeyEvent {
        down_ext(vk, mods, false)
    }

    fn down_ext(vk: u32, mods: KeyMods, extended: bool) -> KeyEvent {
        KeyEvent::Down {
            vk,
            mods,
            repeat: false,
            extended,
        }
    }

    #[test]
    fn gate_passes_everything_until_recording_starts() {
        let mut gate = KeyGate::new();
        assert_eq!(gate.route(down(u32::from(b'M'), CTRL_ALT)), Route::Pass);
        assert_eq!(gate.route(KeyEvent::Up { vk: 0x4D }), Route::Pass);
        assert_eq!(gate.route(KeyEvent::Char), Route::Pass);
    }

    #[test]
    fn gate_swallows_every_key_while_recording_and_reports_the_decisive_one() {
        let mut gate = KeyGate::new();
        assert!(gate.begin());
        assert!(!gate.begin());
        assert_eq!(gate.route(down(VK_CONTROL, CTRL_ALT)), Route::Swallow);
        assert_eq!(gate.route(down(VK_MENU, CTRL_ALT)), Route::Swallow);
        assert_eq!(gate.route(KeyEvent::Up { vk: VK_MENU }), Route::Swallow);
        assert_eq!(gate.route(KeyEvent::Char), Route::Swallow);
        assert!(gate.is_recording());

        let expected = Recorded::Captured(parse_hotkey("Ctrl+Alt+K").unwrap());
        assert_eq!(
            gate.route(down(u32::from(b'K'), CTRL_ALT)),
            Route::Recorded(expected)
        );
        assert!(!gate.is_recording());
    }

    #[test]
    fn gate_keeps_swallowing_the_decisive_key_until_it_is_released() {
        let mut gate = KeyGate::new();
        gate.begin();
        assert_eq!(
            gate.route(down(VK_ESCAPE, KeyMods::default())),
            Route::Recorded(Recorded::Cancelled)
        );
        assert_eq!(gate.route(KeyEvent::Char), Route::Swallow);
        assert_eq!(
            gate.route(KeyEvent::Down {
                vk: VK_ESCAPE,
                mods: KeyMods::default(),
                repeat: true,
                extended: false,
            }),
            Route::Swallow
        );
        assert_eq!(
            gate.route(down(u32::from(b'A'), KeyMods::default())),
            Route::Pass
        );
        assert_eq!(gate.route(KeyEvent::Up { vk: VK_ESCAPE }), Route::Swallow);
        assert_eq!(gate.route(down(VK_ESCAPE, KeyMods::default())), Route::Pass);
    }

    #[test]
    fn gate_stops_draining_on_a_fresh_press_and_on_reset() {
        let mut gate = KeyGate::new();
        gate.begin();
        gate.route(down(0x20, CTRL_ALT));
        assert_eq!(gate.route(down(0x20, CTRL_ALT)), Route::Pass);
        assert_eq!(gate.route(KeyEvent::Char), Route::Pass);

        gate.begin();
        gate.route(down(0x20, CTRL_ALT));
        gate.reset();
        assert!(!gate.is_recording());
        assert_eq!(gate.route(KeyEvent::Char), Route::Pass);

        gate.begin();
        gate.reset();
        assert_eq!(gate.route(down(u32::from(b'M'), CTRL_ALT)), Route::Pass);
    }
}
