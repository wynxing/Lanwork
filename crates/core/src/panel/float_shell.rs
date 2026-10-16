//! 工具悬浮窗外壳的状态。没有界面，也不写数据目录。
//!
//! 可见行为见 `docs/tools.md`「悬浮窗外壳」。贴边几何调用便签悬浮窗已经在用的
//! [`release_float`] 和 [`settle_float`]，不另写一套距离和边。
//! 钉住控件画在哪里、关掉钉住之后是否离开最上层、钉住会不会挡住失焦收回、
//! 没有记下位置时窗口放在哪里、默认大小、重启后会不会自己打开、
//! 能否与便签收纳待办叠在一起、是否显示来源行，规格都还没定。这里不替这些项做决定。
//!
//! 落盘形状留给以后的导出（第 6 步）。相对路径仍待定，所以本模块不把文件放进数据目录，
//! 也不进导出白名单。

use serde::Serialize;
use serde_json::Value;

use crate::notes::{FloatDock, FloatGeometry};
use crate::storage::is_supported_schema;

use super::float_snap::{DockInput, DockState, PxBox, ScreenBounds, edge_label, transition};
use super::note_page::{MonitorWork, release_float, settle_float};

/// 一份外壳记录。三个字段都可以缺，缺了就是还没记过，不是默认值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShellRecord {
    /// 展开后的位置和大小。贴边时仍是展开矩形，不是把手。
    pub geometry: Option<FloatGeometry>,
    pub dock: Option<FloatDock>,
    /// `None` 是没记过。`Some(false)` 会写进文件，和没记过不是一回事。
    pub pinned: Option<bool>,
}

/// 从字节读到的结果。坏 JSON 不当成空记录，避免把一份写坏的钉住状态当成没钉住。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellBytes {
    Record(ShellRecord),
    /// `schemaVersion` 高于当前版本，或不是数字。调用方留着原文，不要写回去。
    UnsupportedSchema,
    Corrupt,
}

/// 第 6 步导出要用的字段。位置、尺寸、贴边、钉住分开，缺的保持缺。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExportSlice {
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub dock: Option<FloatDock>,
    pub pinned: Option<bool>,
}

/// 备份目前只放位置。尺寸、贴边、钉住是否进入备份仍待定，所以不在这里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupSlice {
    pub x: i32,
    pub y: i32,
}

/// 拖动松手或点把手。失焦不在这里：钉住和失焦收回的关系还没定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellGesture {
    DragFinished {
        rect: PxBox,
        moved: bool,
        from_collapsed: bool,
        size_changed: bool,
    },
    StripClick,
}

/// 外壳会话。内容区由调用方换，这里只记位置、大小、贴边和钉住。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellSession {
    expanded: Option<PxBox>,
    dock: Option<FloatDock>,
    pinned: Option<bool>,
}

impl ShellSession {
    #[must_use]
    pub fn from_record(record: ShellRecord) -> Self {
        Self {
            expanded: record.geometry.map(geometry_box),
            dock: record.dock,
            pinned: record.pinned,
        }
    }

    #[must_use]
    pub fn record(self) -> ShellRecord {
        ShellRecord {
            geometry: self.expanded.map(box_geometry),
            dock: self.dock,
            pinned: self.pinned,
        }
    }

    /// 规格第 1 条：始终在其他普通窗口之上。钉住关掉之后会不会离开，还没定，这里不看钉住。
    #[must_use]
    pub fn above_ordinary_windows(self) -> bool {
        true
    }

    /// 规格：窗口不点击穿透。
    #[must_use]
    pub fn click_through(self) -> bool {
        false
    }

    /// 失焦不改贴边。钉住是否阻止收回还没定，不在这一步里选。
    #[must_use]
    pub fn blur_changes_dock(self) -> bool {
        false
    }

    pub fn set_pinned(&mut self, pinned: bool) {
        self.pinned = Some(pinned);
    }

    #[must_use]
    pub fn pinned(self) -> Option<bool> {
        self.pinned
    }

    #[must_use]
    pub fn edge_name(self) -> &'static str {
        edge_label(self.dock)
    }

    #[must_use]
    pub fn collapsed(self) -> bool {
        self.dock.is_some_and(|dock| dock.collapsed)
    }

    /// 调用方已经有一块矩形时放进来。没有记下位置时出现在哪里仍待定，这里不造一块。
    pub fn place(&mut self, expanded: PxBox) {
        self.expanded = Some(expanded);
    }

    /// 按记下的值再打开。没有几何时返回 `None`。贴边对得上时不拉回工作区。
    #[must_use]
    pub fn reopen(
        self,
        screens: &[ScreenBounds],
        monitors: &[MonitorWork],
        fallback: MonitorWork,
    ) -> Option<DockState> {
        let expanded = self.expanded?;
        Some(settle_float(
            DockState {
                expanded,
                dock: self.dock,
            },
            screens,
            monitors,
            fallback,
        ))
    }

    /// 拖动松手或点把手。还没有矩形时不动。
    pub fn apply(
        &mut self,
        gesture: ShellGesture,
        screens: &[ScreenBounds],
        monitors: &[MonitorWork],
        fallback: MonitorWork,
    ) -> Option<DockState> {
        let expanded = self.expanded?;
        let state = DockState {
            expanded,
            dock: self.dock,
        };
        let next = match gesture {
            ShellGesture::DragFinished {
                rect,
                moved,
                from_collapsed,
                size_changed,
            } => {
                release_float(
                    &state,
                    screens,
                    DockInput::DragFinished {
                        rect,
                        moved,
                        from_collapsed,
                        size_changed,
                    },
                    monitors,
                    fallback,
                )
                .state
            }
            ShellGesture::StripClick => transition(&state, screens, DockInput::StripClick).state,
        };
        self.expanded = Some(next.expanded);
        self.dock = next.dock;
        Some(next)
    }
}

#[must_use]
pub fn export_slice(record: &ShellRecord) -> ExportSlice {
    match record.geometry {
        Some(geometry) => ExportSlice {
            x: Some(geometry.x),
            y: Some(geometry.y),
            width: Some(geometry.width),
            height: Some(geometry.height),
            dock: record.dock,
            pinned: record.pinned,
        },
        None => ExportSlice {
            dock: record.dock,
            pinned: record.pinned,
            ..ExportSlice::default()
        },
    }
}

/// 只有位置。没有几何时没有可备份的位置。
#[must_use]
pub fn backup_slice(record: &ShellRecord) -> Option<BackupSlice> {
    record.geometry.map(|geometry| BackupSlice {
        x: geometry.x,
        y: geometry.y,
    })
}

/// 落盘字节。不选择数据目录里的路径。
#[must_use]
pub fn encode(record: &ShellRecord) -> Vec<u8> {
    let on_disk = OnDisk {
        schema_version: crate::storage::SCHEMA_VERSION,
        geometry: record.geometry,
        dock: record.dock.as_ref(),
        pinned: record.pinned,
    };
    let mut bytes = serde_json::to_vec(&on_disk).expect("外壳记录可以编码");
    bytes.push(b'\n');
    bytes
}

#[must_use]
pub fn decode(bytes: &[u8]) -> ShellBytes {
    let value: Value = match serde_json::from_slice(bytes) {
        Ok(value) => value,
        Err(_) => return ShellBytes::Corrupt,
    };
    let Some(obj) = value.as_object() else {
        return ShellBytes::Corrupt;
    };
    match obj.get("schemaVersion") {
        None => {}
        Some(Value::Number(number)) => match number.as_u64() {
            Some(version) if is_supported_schema(u32::try_from(version).unwrap_or(u32::MAX)) => {}
            _ => return ShellBytes::UnsupportedSchema,
        },
        Some(_) => return ShellBytes::UnsupportedSchema,
    }
    let pinned = match obj.get("floatPinned") {
        None => None,
        Some(Value::Bool(pinned)) => Some(*pinned),
        Some(_) => return ShellBytes::Corrupt,
    };
    ShellBytes::Record(ShellRecord {
        geometry: obj.get("floatWindow").and_then(geometry_of),
        dock: obj.get("floatDock").and_then(FloatDock::from_json),
        pinned,
    })
}

/// 导出用的 JSON。尺寸、贴边、钉住和位置都在；缺的不写。
#[must_use]
pub fn export_json(record: &ShellRecord) -> Vec<u8> {
    let slice = export_slice(record);
    let on_disk = ExportOnDisk {
        x: slice.x,
        y: slice.y,
        width: slice.width,
        height: slice.height,
        dock: slice.dock.as_ref(),
        pinned: slice.pinned,
    };
    serde_json::to_vec(&on_disk).expect("导出字段可以编码")
}

/// 备份用的 JSON。只有位置。尺寸、贴边、钉住不出现。
#[must_use]
pub fn backup_json(record: &ShellRecord) -> Option<Vec<u8>> {
    let slice = backup_slice(record)?;
    serde_json::to_vec(&BackupOnDisk {
        x: slice.x,
        y: slice.y,
    })
    .ok()
}

#[derive(Serialize)]
struct OnDisk<'a> {
    #[serde(rename = "schemaVersion")]
    schema_version: u32,
    #[serde(rename = "floatWindow", skip_serializing_if = "Option::is_none")]
    geometry: Option<FloatGeometry>,
    #[serde(rename = "floatDock", skip_serializing_if = "Option::is_none")]
    dock: Option<&'a FloatDock>,
    #[serde(rename = "floatPinned", skip_serializing_if = "Option::is_none")]
    pinned: Option<bool>,
}

#[derive(Serialize)]
struct ExportOnDisk<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    x: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    y: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    height: Option<u32>,
    #[serde(rename = "floatDock", skip_serializing_if = "Option::is_none")]
    dock: Option<&'a FloatDock>,
    #[serde(rename = "floatPinned", skip_serializing_if = "Option::is_none")]
    pinned: Option<bool>,
}

#[derive(Serialize)]
struct BackupOnDisk {
    x: i32,
    y: i32,
}

fn geometry_of(value: &Value) -> Option<FloatGeometry> {
    let obj = value.as_object()?;
    Some(FloatGeometry {
        x: i32::try_from(obj.get("x")?.as_i64()?).ok()?,
        y: i32::try_from(obj.get("y")?.as_i64()?).ok()?,
        width: u32::try_from(obj.get("width")?.as_u64()?).ok()?,
        height: u32::try_from(obj.get("height")?.as_u64()?).ok()?,
    })
}

fn geometry_box(geometry: FloatGeometry) -> PxBox {
    PxBox {
        x: geometry.x,
        y: geometry.y,
        width: geometry.width,
        height: geometry.height,
    }
}

fn box_geometry(rect: PxBox) -> FloatGeometry {
    FloatGeometry {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notes::DockEdge;
    use crate::shell::WorkArea;

    fn geometry() -> FloatGeometry {
        FloatGeometry {
            x: 12,
            y: 34,
            width: 320,
            height: 360,
        }
    }

    fn dock(collapsed: bool) -> FloatDock {
        FloatDock {
            edge: DockEdge::Left,
            offset: 80,
            collapsed,
        }
    }

    fn full_record() -> ShellRecord {
        ShellRecord {
            geometry: Some(geometry()),
            dock: Some(dock(true)),
            pinned: Some(true),
        }
    }

    fn area(left: i32, top: i32, right: i32, bottom: i32) -> WorkArea {
        WorkArea {
            left,
            top,
            right,
            bottom,
        }
    }

    fn monitor(bounds: WorkArea, work: WorkArea) -> MonitorWork {
        MonitorWork {
            bounds,
            area: work,
            dpi: 96,
        }
    }

    fn screen(bounds: WorkArea) -> ScreenBounds {
        ScreenBounds {
            left: bounds.left,
            top: bounds.top,
            right: bounds.right,
            bottom: bounds.bottom,
            dpi: 96,
        }
    }

    /// 任务栏在底边。左、右、上三条工作区边和显示器外沿重合。
    fn bottom_taskbar() -> (ScreenBounds, MonitorWork) {
        let bounds = area(0, 0, 1920, 1080);
        let work = area(0, 0, 1920, 1040);
        (screen(bounds), monitor(bounds, work))
    }

    fn text(bytes: &[u8]) -> String {
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[test]
    fn round_trip_keeps_size_dock_and_pin_separate_from_a_missing_record() {
        let encoded = encode(&full_record());
        let ShellBytes::Record(decoded) = decode(&encoded) else {
            panic!("应读回记录");
        };
        assert_eq!(decoded, full_record());
        let raw = text(&encoded);
        assert!(raw.contains("\"width\":320"));
        assert!(raw.contains("\"height\":360"));
        assert!(raw.contains("\"floatDock\""));
        assert!(raw.contains("\"floatPinned\":true"));
        assert!(raw.contains("\"collapsed\":true"));

        let cleared = encode(&ShellRecord::default());
        assert_eq!(text(&cleared), "{\"schemaVersion\":1}\n");
        let ShellBytes::Record(empty) = decode(&cleared) else {
            panic!("空记录");
        };
        assert_eq!(empty, ShellRecord::default());
    }

    #[test]
    fn explicit_unpin_is_not_the_same_as_never_recorded() {
        let mut record = full_record();
        record.pinned = Some(false);
        let ShellBytes::Record(decoded) = decode(&encode(&record)) else {
            panic!("应读回");
        };
        assert_eq!(decoded.pinned, Some(false));
        let mut raw = encode(&record);
        raw.pop();
        let value: Value = serde_json::from_slice(&raw).unwrap();
        assert_eq!(value["floatPinned"], false);
    }

    #[test]
    fn export_keeps_size_dock_and_pin_and_backup_keeps_only_position() {
        let exported = text(&export_json(&full_record()));
        assert!(exported.contains("\"x\":12"));
        assert!(exported.contains("\"y\":34"));
        assert!(exported.contains("\"width\":320"));
        assert!(exported.contains("\"height\":360"));
        assert!(exported.contains("\"floatDock\""));
        assert!(exported.contains("\"floatPinned\":true"));

        let backed = text(&backup_json(&full_record()).unwrap());
        assert_eq!(backed, "{\"x\":12,\"y\":34}");
        assert!(!backed.contains("width"));
        assert!(!backed.contains("height"));
        assert!(!backed.contains("floatDock"));
        assert!(!backed.contains("floatPinned"));
        assert!(!backed.contains("collapsed"));

        let pin_only = ShellRecord {
            pinned: Some(true),
            ..ShellRecord::default()
        };
        assert!(backup_json(&pin_only).is_none());
        let exported = text(&export_json(&pin_only));
        assert!(exported.contains("\"floatPinned\":true"));
        assert!(!exported.contains("width"));
    }

    #[test]
    fn bad_edge_drops_only_the_dock_and_a_bad_pin_is_corrupt() {
        let raw = br#"{"schemaVersion":1,"floatWindow":{"x":1,"y":2,"width":3,"height":4},"floatDock":{"edge":"bottom","offset":1,"collapsed":true},"floatPinned":true}"#;
        let ShellBytes::Record(record) = decode(raw) else {
            panic!("底边不是贴边，其余仍在");
        };
        assert!(record.dock.is_none());
        assert_eq!(record.geometry.unwrap().width, 3);
        assert_eq!(record.pinned, Some(true));

        let bad_pin = br#"{"schemaVersion":1,"floatPinned":"yes"}"#;
        assert_eq!(decode(bad_pin), ShellBytes::Corrupt);
        assert_eq!(decode(b"[]"), ShellBytes::Corrupt);
        assert_eq!(decode(b"{"), ShellBytes::Corrupt);
        assert_eq!(
            decode(br#"{"schemaVersion":2,"floatPinned":true}"#),
            ShellBytes::UnsupportedSchema
        );
        assert_eq!(
            decode(br#"{"schemaVersion":"1"}"#),
            ShellBytes::UnsupportedSchema
        );
        let ShellBytes::Record(legacy) = decode(br#"{"floatPinned":false}"#) else {
            panic!("缺少 schemaVersion 按当前版本读");
        };
        assert_eq!(legacy.pinned, Some(false));
    }

    #[test]
    fn dragging_to_an_outer_edge_collapses_and_keeps_pin() {
        let (display, primary) = bottom_taskbar();
        let mut session = ShellSession::from_record(ShellRecord {
            pinned: Some(true),
            ..ShellRecord::default()
        });
        assert!(
            session
                .apply(
                    ShellGesture::DragFinished {
                        rect: PxBox {
                            x: -80,
                            y: 100,
                            width: 320,
                            height: 360,
                        },
                        moved: true,
                        from_collapsed: false,
                        size_changed: false,
                    },
                    &[display],
                    &[primary],
                    primary,
                )
                .is_none(),
            "没有矩形时不替窗口选位置"
        );
        session.place(PxBox {
            x: 400,
            y: 120,
            width: 320,
            height: 360,
        });
        let next = session
            .apply(
                ShellGesture::DragFinished {
                    rect: PxBox {
                        x: -80,
                        y: 100,
                        width: 320,
                        height: 360,
                    },
                    moved: true,
                    from_collapsed: false,
                    size_changed: false,
                },
                &[display],
                &[primary],
                primary,
            )
            .unwrap();
        let dock = next.dock.expect("越出左边后贴上");
        assert_eq!(dock.edge, DockEdge::Left);
        assert!(dock.collapsed);
        assert_eq!(session.pinned(), Some(true));
        assert_eq!(session.edge_name(), "left");
        assert!(session.collapsed());
        assert!(session.above_ordinary_windows());
        assert!(!session.click_through());
    }

    #[test]
    fn an_undocked_release_stays_in_the_work_area_and_a_dock_is_not_pulled_back() {
        let (display, primary) = bottom_taskbar();
        let mut session = ShellSession::from_record(ShellRecord {
            geometry: Some(FloatGeometry {
                x: 400,
                y: 120,
                width: 320,
                height: 360,
            }),
            pinned: Some(false),
            ..ShellRecord::default()
        });
        let next = session
            .apply(
                ShellGesture::DragFinished {
                    rect: PxBox {
                        x: 400,
                        y: 900,
                        width: 320,
                        height: 360,
                    },
                    moved: true,
                    from_collapsed: false,
                    size_changed: false,
                },
                &[display],
                &[primary],
                primary,
            )
            .unwrap();
        assert!(next.dock.is_none(), "底边不贴");
        assert!(next.expanded.x + 320 <= 1920);
        assert!(next.expanded.y + 360 <= 1040);
        assert_eq!(session.pinned(), Some(false));
        assert!(session.above_ordinary_windows());

        let parked = ShellSession::from_record(ShellRecord {
            geometry: Some(FloatGeometry {
                x: 0,
                y: 0,
                width: 320,
                height: 360,
            }),
            dock: Some(FloatDock {
                edge: DockEdge::Top,
                offset: 100,
                collapsed: false,
            }),
            pinned: Some(true),
        });
        let opened = parked.reopen(&[display], &[primary], primary).unwrap();
        assert_eq!(opened.dock.unwrap().edge, DockEdge::Top);
        assert_eq!(opened.expanded.y, 0, "贴边时不拉回工作区");
    }

    #[test]
    fn resizing_does_not_dock_and_a_strip_click_toggles_without_blur() {
        let (display, primary) = bottom_taskbar();
        let mut session = ShellSession::from_record(ShellRecord {
            geometry: Some(FloatGeometry {
                x: 0,
                y: 80,
                width: 320,
                height: 360,
            }),
            dock: Some(dock(false)),
            pinned: Some(true),
        });
        let resized = session
            .apply(
                ShellGesture::DragFinished {
                    rect: PxBox {
                        x: 0,
                        y: 80,
                        width: 400,
                        height: 360,
                    },
                    moved: false,
                    from_collapsed: false,
                    size_changed: true,
                },
                &[display],
                &[primary],
                primary,
            )
            .unwrap();
        assert!(resized.dock.is_some());
        assert!(!resized.dock.unwrap().collapsed);
        assert_eq!(resized.expanded.width, 400);

        let toggled = session
            .apply(ShellGesture::StripClick, &[display], &[primary], primary)
            .unwrap();
        assert!(toggled.dock.unwrap().collapsed);
        let before = session.record();
        assert!(!session.blur_changes_dock());
        assert_eq!(session.record(), before);

        session.set_pinned(false);
        assert_eq!(session.pinned(), Some(false));
        assert!(session.collapsed());
        assert!(session.above_ordinary_windows());
        assert_eq!(session.record().geometry.unwrap().width, 400);
    }

    #[test]
    fn a_collapsed_press_that_does_not_move_expands() {
        let (display, primary) = bottom_taskbar();
        let mut session = ShellSession::from_record(ShellRecord {
            geometry: Some(FloatGeometry {
                x: 0,
                y: 80,
                width: 320,
                height: 360,
            }),
            dock: Some(dock(true)),
            pinned: Some(false),
        });
        let next = session
            .apply(
                ShellGesture::DragFinished {
                    rect: PxBox {
                        x: 0,
                        y: 80,
                        width: 3,
                        height: 40,
                    },
                    moved: false,
                    from_collapsed: true,
                    size_changed: false,
                },
                &[display],
                &[primary],
                primary,
            )
            .unwrap();
        assert!(!next.dock.expect("仍贴着").collapsed);
        assert_eq!(session.pinned(), Some(false));
        assert!(session.above_ordinary_windows());
    }

    #[test]
    fn reopen_without_a_saved_rect_does_not_invent_one() {
        let (display, primary) = bottom_taskbar();
        let session = ShellSession::from_record(ShellRecord {
            pinned: Some(true),
            ..ShellRecord::default()
        });
        assert!(session.reopen(&[display], &[primary], primary).is_none());
    }

    #[test]
    fn a_saved_rect_smaller_than_the_shared_minimum_grows_on_reopen() {
        let (display, primary) = bottom_taskbar();
        let session = ShellSession::from_record(ShellRecord {
            geometry: Some(FloatGeometry {
                x: 10,
                y: 10,
                width: 50,
                height: 50,
            }),
            ..ShellRecord::default()
        });
        let opened = session.reopen(&[display], &[primary], primary).unwrap();
        assert_eq!(
            (opened.expanded.width, opened.expanded.height),
            (200, 120),
            "沿用工作区函数里已有的最小尺寸，不是另定的默认大小"
        );
    }
}
