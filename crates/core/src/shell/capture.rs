//! 设置页录入热键：把一次按键（虚拟键码和修饰键）换成 [`Hotkey`]。
//!
//! 不调用 Win32。按键从哪里来、录入期间怎样让已注册的全局热键不抢键，在 `crates/app`。
//! 能录入的键就是 [`crate::config::parse_hotkey`] 接受的键：`A`–`Z`、`0`–`9`、`F1`–`F24`。
//! 录入比保存时的校验多一条：字母和数字必须带修饰键，功能键可以单独使用。
//! 产品规格没有写，这是实现选择，见架构文档「设置页」。

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
const VK_F1: u32 = 0x70;
const VK_F24: u32 = 0x87;

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
    /// 不能作为热键的键：不在键名表里，或字母、数字没带修饰键。
    Rejected,
}

fn is_modifier_key(vk: u32) -> bool {
    matches!(vk, VK_SHIFT | VK_CONTROL | VK_MENU | VK_LWIN | VK_RWIN)
        || (VK_LSHIFT..=VK_RMENU).contains(&vk)
}

/// 把一次按下换成录入结果。`vk` 是 Win32 虚拟键码。
#[must_use]
pub fn capture_step(vk: u32, mods: KeyMods) -> CaptureStep {
    if is_modifier_key(vk) || vk == VK_PROCESSKEY {
        return CaptureStep::Waiting;
    }
    if vk == VK_ESCAPE && !mods.any() {
        return CaptureStep::Cancelled;
    }
    let key = match vk {
        0x41..=0x5A => HotkeyKey::Letter(vk as u8),
        0x30..=0x39 => HotkeyKey::Digit(vk as u8),
        VK_F1..=VK_F24 => HotkeyKey::Function((vk - VK_F1 + 1) as u8),
        _ => return CaptureStep::Rejected,
    };
    if !mods.any() && !matches!(key, HotkeyKey::Function(_)) {
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
    pub fn feed(&mut self, vk: u32, mods: KeyMods) -> Option<Recorded> {
        if !self.recording {
            return None;
        }
        let recorded = match capture_step(vk, mods) {
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
                KeyEvent::Down { vk, mods, .. } => match self.recorder.feed(vk, mods) {
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

    const CTRL_ALT: KeyMods = KeyMods {
        control: true,
        alt: true,
        shift: false,
        win: false,
    };

    fn captured(vk: u32, mods: KeyMods) -> String {
        match capture_step(vk, mods) {
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
        let keys = (0x30..=0x39).chain(0x41..=0x5A).chain(VK_F1..=VK_F24);
        for vk in keys {
            for item in mods {
                if let CaptureStep::Captured(hotkey) = capture_step(vk, item) {
                    let parsed = parse_hotkey(&hotkey.canonical()).unwrap();
                    assert_eq!(parsed, hotkey);
                    assert_eq!(parsed.virtual_key(), vk);
                }
            }
        }
    }

    #[test]
    fn function_keys_work_alone_but_letters_and_digits_need_a_modifier() {
        assert_eq!(captured(VK_F1, KeyMods::default()), "F1");
        assert_eq!(captured(VK_F24, KeyMods::default()), "F24");
        assert_eq!(
            capture_step(u32::from(b'M'), KeyMods::default()),
            CaptureStep::Rejected
        );
        assert_eq!(
            capture_step(u32::from(b'5'), KeyMods::default()),
            CaptureStep::Rejected
        );
        assert_eq!(
            capture_step(VK_F24 + 1, KeyMods::default()),
            CaptureStep::Rejected
        );
    }

    #[test]
    fn keys_outside_the_table_are_rejected() {
        for vk in [0x20, 0x0D, 0x09, 0x25, 0x60, 0xBA, 0xDB, 0x2E] {
            assert_eq!(capture_step(vk, CTRL_ALT), CaptureStep::Rejected, "{vk:#x}");
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
            assert_eq!(capture_step(vk, CTRL_ALT), CaptureStep::Waiting, "{vk:#x}");
        }
    }

    #[test]
    fn escape_cancels_only_without_a_modifier() {
        assert_eq!(
            capture_step(VK_ESCAPE, KeyMods::default()),
            CaptureStep::Cancelled
        );
        assert_eq!(capture_step(VK_ESCAPE, CTRL_ALT), CaptureStep::Rejected);
    }

    #[test]
    fn recorder_ends_on_the_first_decisive_key_and_ignores_later_ones() {
        let mut recorder = HotkeyRecorder::new();
        assert_eq!(recorder.feed(u32::from(b'M'), CTRL_ALT), None);
        assert!(recorder.start());
        assert!(!recorder.start());
        assert!(recorder.is_recording());
        assert_eq!(recorder.feed(VK_CONTROL, CTRL_ALT), None);
        assert_eq!(recorder.feed(VK_MENU, CTRL_ALT), None);
        assert!(recorder.is_recording());

        let done = recorder.feed(u32::from(b'N'), CTRL_ALT);
        assert_eq!(
            done,
            Some(Recorded::Captured(parse_hotkey("Ctrl+Alt+N").unwrap()))
        );
        assert!(!recorder.is_recording());
        assert_eq!(recorder.feed(u32::from(b'K'), CTRL_ALT), None);
    }

    #[test]
    fn recorder_reports_cancel_and_rejection_and_can_start_again() {
        let mut recorder = HotkeyRecorder::new();
        recorder.start();
        assert_eq!(
            recorder.feed(VK_ESCAPE, KeyMods::default()),
            Some(Recorded::Cancelled)
        );
        assert!(!recorder.is_recording());

        recorder.start();
        assert_eq!(recorder.feed(0x20, CTRL_ALT), Some(Recorded::Rejected));
        assert!(!recorder.is_recording());

        recorder.start();
        recorder.stop();
        assert_eq!(recorder.feed(u32::from(b'M'), CTRL_ALT), None);
        assert!(recorder.start());
    }

    fn down(vk: u32, mods: KeyMods) -> KeyEvent {
        KeyEvent::Down {
            vk,
            mods,
            repeat: false,
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
                repeat: true
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
