//! 热角检测的纯逻辑。
//!
//! 不安装钩子，也不读光标。Windows 上的两种检测方案在 `plat` 里。

mod logic;

#[cfg(windows)]
pub mod plat;

pub use logic::{
    Corner, Dwell, Monitor, MonitorPick, PxRect, aim_point, corner_point, in_corner, outside_point,
    point_in_hot_corner, select_monitor,
};

#[cfg(windows)]
pub use plat::{RunOpts, SchemeKind};
