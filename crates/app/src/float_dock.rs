//! 便签和收纳悬浮窗的贴边会话。几何在 `lanwork_core::panel`，这里只接窗口和计时。
//!
//! 滑出每步多久、拖动结束后留几轮再接受失焦，规格没有写。下面的数是当前做法，待定。

use std::time::Duration;

use lanwork_core::notes::{FloatDock, FloatGeometry};
use lanwork_core::panel::{
    ANIM_STEPS, DockInput, DockState, PxBox, ScreenBounds, dock_applies, dpi_at, edge_label,
    lerp_box, moved_enough, restore_float, transition, visible_box,
};
use lanwork_core::shell::WorkArea;
use windows::Win32::Foundation::HWND;

use crate::bar_win;

/// 滑出或收回时，每一步的间隔。一共 [`ANIM_STEPS`] 步。待定。
pub(crate) const FRAME_MS: u64 = 20;

/// 关掉窗口或退出时写进文件的展开矩形和贴边。不是细条的矩形。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SavedFloat {
    pub geometry: FloatGeometry,
    pub dock: Option<FloatDock>,
}

/// 一次贴边变化。`animate` 为真时从 `from` 滑到 `to`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Motion {
    pub persist: bool,
    pub animate: bool,
    pub from: PxBox,
    pub to: PxBox,
}

/// 窗口上要跟着改的几个属性。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Chrome {
    pub thin: bool,
    pub collapsed: bool,
    pub edge: &'static str,
}

struct DragFrom {
    origin: (i32, i32),
    size: (u32, u32),
    collapsed: bool,
}

struct Anim {
    from: PxBox,
    to: PxBox,
    step: u32,
}

pub(crate) struct Session {
    state: DockState,
    anim: Option<Anim>,
    drag_from: Option<DragFrom>,
    dragging: bool,
    hold_blur: bool,
    hold_generation: u8,
}

impl Session {
    pub(crate) fn new(expanded: PxBox, dock: Option<FloatDock>) -> Self {
        Self {
            state: DockState { expanded, dock },
            anim: None,
            drag_from: None,
            dragging: false,
            hold_blur: false,
            hold_generation: 0,
        }
    }

    pub(crate) fn saved(&self) -> SavedFloat {
        let expanded = self.state.expanded;
        SavedFloat {
            geometry: FloatGeometry {
                x: expanded.x,
                y: expanded.y,
                width: expanded.width,
                height: expanded.height,
            },
            dock: self.state.dock,
        }
    }

    pub(crate) fn chrome(&self) -> Chrome {
        let dock_collapsed = self.state.dock.is_some_and(|dock| dock.collapsed);
        Chrome {
            thin: self.anim.is_some() || dock_collapsed,
            collapsed: dock_collapsed && self.anim.is_none(),
            edge: edge_label(self.state.dock),
        }
    }

    pub(crate) fn visible(&self) -> PxBox {
        visible_box(&self.state, &screens())
    }

    /// 记下按下时的矩形。拖动循环里的失焦先被挡住。
    pub(crate) fn begin_drag(&mut self, origin: (i32, i32), size: (u32, u32)) {
        let collapsed = self.state.dock.is_some_and(|dock| dock.collapsed) && self.anim.is_none();
        self.anim = None;
        self.dragging = true;
        self.drag_from = Some(DragFrom {
            origin,
            size,
            collapsed,
        });
    }

    pub(crate) fn end_drag(&mut self, rect: PxBox) -> Motion {
        self.dragging = false;
        self.arm_hold();
        let screens = screens();
        let (moved, from_collapsed, size_changed) = match self.drag_from.take() {
            Some(start) => {
                let dpi = dpi_at(&screens, rect.x, rect.y);
                (
                    moved_enough(start.origin, (rect.x, rect.y), dpi),
                    start.collapsed,
                    start.size != (rect.width, rect.height),
                )
            }
            None => (true, false, false),
        };
        let mut motion = self.apply(
            &screens,
            DockInput::DragFinished {
                rect,
                moved,
                from_collapsed,
                size_changed,
            },
        );
        // 松手时窗口已经在 `rect`。几何模块给的起点是拖动前的位置，滑出要从松手的地方开始。
        if motion.animate {
            motion.from = rect;
            if let Some(anim) = &mut self.anim {
                anim.from = rect;
            }
            if motion.from == motion.to {
                motion.animate = false;
                self.anim = None;
            }
        }
        motion
    }

    pub(crate) fn strip_click(&mut self) -> Motion {
        self.apply(&screens(), DockInput::StripClick)
    }

    /// 拖动、滑动或刚松手时不收起。没有贴边、已经是细条、这条边对不上时，几何模块也不改状态。
    pub(crate) fn blur(&mut self) -> Option<Motion> {
        if self.dragging || self.anim.is_some() || self.hold_blur {
            return None;
        }
        let motion = self.apply(&screens(), DockInput::Blur);
        motion.persist.then_some(motion)
    }

    pub(crate) fn hold_generation(&self) -> u8 {
        self.hold_generation
    }

    pub(crate) fn release_hold(&mut self, generation: u8) {
        if self.hold_generation == generation {
            self.hold_blur = false;
        }
    }

    /// 下一步的矩形。没有在滑时返回 `None`。走完最后一步后不再处于滑动中。
    pub(crate) fn next_frame(&mut self) -> Option<PxBox> {
        let anim = self.anim.as_mut()?;
        anim.step = anim.step.saturating_add(1);
        let frame = lerp_box(anim.from, anim.to, anim.step, ANIM_STEPS);
        if anim.step >= ANIM_STEPS {
            self.anim = None;
        }
        Some(frame)
    }

    pub(crate) fn animating(&self) -> bool {
        self.anim.is_some()
    }

    /// 丢掉这一次拖动，几何保持按下之前的样子。
    pub(crate) fn abandon_drag(&mut self) {
        self.dragging = false;
        self.drag_from = None;
        self.arm_hold();
    }

    /// 换成另一份展开矩形和贴边。叠页切换、并入时用。
    pub(crate) fn replace(&mut self, expanded: PxBox, dock: Option<FloatDock>) {
        self.anim = None;
        self.dragging = false;
        self.drag_from = None;
        self.state = DockState { expanded, dock };
    }

    fn arm_hold(&mut self) {
        self.hold_blur = true;
        self.hold_generation = self.hold_generation.wrapping_add(1);
    }

    fn apply(&mut self, screens: &[ScreenBounds], input: DockInput) -> Motion {
        let before = self.state;
        let change = transition(&before, screens, input);
        self.state = change.state;
        if change.animate {
            self.anim = Some(Anim {
                from: change.from,
                to: change.to,
                step: 0,
            });
        } else {
            self.anim = None;
        }
        Motion {
            persist: change.state != before,
            animate: change.animate,
            from: change.from,
            to: change.to,
        }
    }
}

/// 有记下的贴边、并且这条边在当前屏幕上还对得上时，按贴边打开，不把窗口拉回工作区。
/// 对不上（例如那块屏幕已经不在）时丢掉这一次的贴边，按原来的规则放回工作区。
/// 关掉窗口时会把丢掉贴边的结果写回去。拔掉显示器之后要不要一直忘掉，规格没有写，待定。
pub(crate) fn session_for_open(
    saved: Option<FloatGeometry>,
    dock: Option<FloatDock>,
    default_box: PxBox,
    areas: &[WorkArea],
    fallback: WorkArea,
    min: (u32, u32),
) -> Session {
    let Some(saved) = saved else {
        return Session::new(default_box, None);
    };
    let expanded = PxBox {
        x: saved.x,
        y: saved.y,
        width: saved.width,
        height: saved.height,
    };
    if dock.is_some() && dock_applies(&DockState { expanded, dock }, &screens()) {
        return Session::new(expanded, dock);
    }
    let placed = restore_float(saved, areas, fallback, min);
    Session::new(
        PxBox {
            x: placed.origin.0,
            y: placed.origin.1,
            width: placed.size.0,
            height: placed.size.1,
        },
        None,
    )
}

pub(crate) fn screens() -> Vec<ScreenBounds> {
    bar_win::monitors()
        .into_iter()
        .map(|reading| ScreenBounds {
            left: reading.bounds.left,
            top: reading.bounds.top,
            right: reading.bounds.right,
            bottom: reading.bounds.bottom,
            dpi: reading.dpi,
        })
        .collect()
}

pub(crate) fn place(window: &slint::Window, hwnd: Option<HWND>, rect: PxBox) {
    let width = rect.width.max(1);
    let height = rect.height.max(1);
    window.set_size(slint::PhysicalSize::new(width, height));
    window.set_position(slint::PhysicalPosition::new(rect.x, rect.y));
    if let Some(hwnd) = hwnd {
        bar_win::place_topmost(hwnd, rect.x, rect.y, width, height);
    }
}

pub(crate) fn frame_interval() -> Duration {
    Duration::from_millis(FRAME_MS)
}
