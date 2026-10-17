//! 工具悬浮窗外壳的状态，以及数据目录里的那一份记录。
//!
//! 可见行为见 `docs/tools.md`「悬浮窗外壳」。贴边几何调用便签悬浮窗已经在用的
//! [`release_float`] 和 [`settle_float`]，不另写一套距离和边。
//! 记录在数据目录的 `tool-windows/<工具 id>.json`，进入导出和备份。
//! 包里的这份不含 `open`，恢复后窗口不因备份自动打开。
//! 没有 `floatStack`：不与便签、收纳、待办叠在一起。

use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::notes::{DockEdge, FloatDock, FloatGeometry};
use crate::shell::{WorkArea, to_physical};
use crate::storage::is_supported_schema;

use super::float_snap::{DockInput, DockState, PxBox, ScreenBounds, edge_label, transition};
use super::note_page::{
    FLOAT_MIN_HEIGHT, FLOAT_MIN_WIDTH, MonitorWork, release_float, settle_float,
};

/// 数据目录下的相对目录。每个工具一个 `<id>.json`。
pub const RECORD_DIR: &str = "tool-windows";
/// 清单没有声明大小时的逻辑宽。与便签悬浮窗相同。
pub const DEFAULT_LOGICAL_WIDTH: u32 = 320;
/// 清单没有声明大小时的逻辑高。与便签悬浮窗相同。
pub const DEFAULT_LOGICAL_HEIGHT: u32 = 360;
/// 没有记下位置时，与主屏工作区右缘、下缘的空隙，逻辑像素。
pub const MARGIN_LOGICAL: u32 = 24;

/// 一份外壳记录。几何、贴边和钉住都可以缺，缺了就是还没记过，不是默认值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShellRecord {
    /// 展开后的位置和大小。贴边时仍是展开矩形，不是把手。
    pub geometry: Option<FloatGeometry>,
    pub dock: Option<FloatDock>,
    /// `None` 是没记过，按钉住开。`Some(false)` 会写进文件，和没记过不是一回事。
    pub pinned: Option<bool>,
    /// 退出时窗口开着。缺了按关。
    pub open: bool,
}

/// 从字节读到的结果。坏 JSON 不当成空记录，避免把一份写坏的钉住状态当成没钉住。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellBytes {
    Record(ShellRecord),
    /// `floatWindow` 这个字段在，但坐标读不出来。贴边、钉住和 `open` 仍用文件里的值。
    /// 几何是 `None`。调用方打开时不要把合成的位置写回这份文件。
    GeometryUnreadable(ShellRecord),
    /// `schemaVersion` 高于当前版本，或不是数字。调用方留着原文，不要写回去。
    UnsupportedSchema,
    Corrupt,
}

impl ShellBytes {
    /// 能按这份记录打开窗口。坏 JSON 和更高的 schema 不行。
    /// `floatWindow` 读不出时记录仍在，几何是 `None`，贴边等其他字段还在。
    #[must_use]
    pub fn openable(self) -> Option<ShellRecord> {
        match self {
            Self::Record(record) | Self::GeometryUnreadable(record) => Some(record),
            Self::UnsupportedSchema | Self::Corrupt => None,
        }
    }

    /// 这次打开不要覆盖原文件。读不出的 `floatWindow` 留在磁盘上，直到用户改窗口。
    #[must_use]
    pub fn keep_file_on_open(self) -> bool {
        matches!(self, Self::GeometryUnreadable(_))
    }
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

/// 备份和导出是同一套字段。没有几何时，缺的位置和尺寸仍缺。
pub type BackupSlice = ExportSlice;

/// 拖动松手、点把手或失焦。失焦收回不看钉住。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellGesture {
    DragFinished {
        rect: PxBox,
        moved: bool,
        from_collapsed: bool,
        size_changed: bool,
    },
    StripClick,
    Blur,
}

/// 外壳会话。内容区由调用方换，这里只记位置、大小、贴边和钉住。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellSession {
    expanded: Option<PxBox>,
    dock: Option<FloatDock>,
    pinned: Option<bool>,
    open: bool,
}

impl ShellSession {
    #[must_use]
    pub fn from_record(record: ShellRecord) -> Self {
        Self {
            expanded: record.geometry.map(geometry_box),
            dock: record.dock,
            pinned: record.pinned,
            open: record.open,
        }
    }

    #[must_use]
    pub fn record(self) -> ShellRecord {
        ShellRecord {
            geometry: self.expanded.map(box_geometry),
            dock: self.dock,
            pinned: self.pinned,
            open: self.open,
        }
    }

    /// 钉住开，或还没记过：在其他普通窗口之上。明确关掉之后离开最上层。
    #[must_use]
    pub fn above_ordinary_windows(self) -> bool {
        self.pinned != Some(false)
    }

    /// 规格：窗口不点击穿透。
    #[must_use]
    pub fn click_through(self) -> bool {
        false
    }

    pub fn set_open(&mut self, open: bool) {
        self.open = open;
    }

    #[must_use]
    pub fn open(self) -> bool {
        self.open
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

    /// 调用方已经有一块矩形时放进来。
    /// 没有记下位置时的默认位置是 [`shell_default_origin`]：主屏工作区右下角，留 [`MARGIN_LOGICAL`] 逻辑像素。这里不另造一块。
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
            ShellGesture::Blur => transition(&state, screens, DockInput::Blur).state,
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

/// 和 [`export_slice`] 同一套字段。
#[must_use]
pub fn backup_slice(record: &ShellRecord) -> BackupSlice {
    export_slice(record)
}

/// 清单声明的逻辑尺寸。没有声明时用 320×360。小于 200×120 时抬到最小尺寸。
#[must_use]
pub fn shell_logical_size(declared: Option<(u32, u32)>) -> (u32, u32) {
    let (width, height) = declared.unwrap_or((DEFAULT_LOGICAL_WIDTH, DEFAULT_LOGICAL_HEIGHT));
    (width.max(FLOAT_MIN_WIDTH), height.max(FLOAT_MIN_HEIGHT))
}

/// 主屏工作区右下角，留 [`MARGIN_LOGICAL`] 逻辑像素。窗口比工作区还大时，原点停在工作区左上角。
#[must_use]
pub fn shell_default_origin(work: WorkArea, width_px: i32, height_px: i32, dpi: u32) -> (i32, i32) {
    let margin = to_physical(MARGIN_LOGICAL, dpi).max(0);
    let x = work
        .right
        .saturating_sub(width_px.max(0))
        .saturating_sub(margin);
    let y = work
        .bottom
        .saturating_sub(height_px.max(0))
        .saturating_sub(margin);
    (x.max(work.left), y.max(work.top))
}

/// 打开时放进贴边会话的几何。
///
/// `floatWindow` 不在时，返回记下的几何，可以是 `None`，调用方再用 [`shell_default_origin`]。
/// 字段在但读不出时，有贴边就造一块贴在 `screen` 外边上的展开矩形，让 `saved.and(dock)` 留住贴边；
/// 没有贴边时返回 `None`。这块矩形只用于这一次显示，[`ShellBytes::keep_file_on_open`] 为真时不要写回。
#[must_use]
pub fn geometry_for_open(
    record: &ShellRecord,
    geometry_unreadable: bool,
    size: (u32, u32),
    screen: ScreenBounds,
) -> Option<FloatGeometry> {
    if geometry_unreadable {
        return record
            .dock
            .map(|dock| edge_anchor(dock, size.0, size.1, screen));
    }
    record.geometry
}

/// 写盘失败时窗口上的那一行。原文保留由原子替换保证，这里只给界面用。
#[must_use]
pub fn write_failure_text(err: &impl std::fmt::Display) -> String {
    format!("保存失败：{err}")
}

fn edge_anchor(dock: FloatDock, width: u32, height: u32, screen: ScreenBounds) -> FloatGeometry {
    let width = width.max(1);
    let height = height.max(1);
    let (x, y) = match dock.edge {
        DockEdge::Left => (screen.left, dock.offset),
        DockEdge::Right => (screen.right.saturating_sub_unsigned(width), dock.offset),
        DockEdge::Top => (dock.offset, screen.top),
    };
    FloatGeometry {
        x,
        y,
        width,
        height,
    }
}

/// `tool-windows/<id>.json`。id 不是单个路径分量时返回 `None`。
#[must_use]
pub fn record_relative(tool_id: &str) -> Option<PathBuf> {
    if crate::storage::validate_id(tool_id).is_err() {
        return None;
    }
    Some(PathBuf::from(RECORD_DIR).join(format!("{tool_id}.json")))
}

/// 读一份记录。文件不存在时是空记录，不是错误。
pub fn read_record(data_dir: &Path, tool_id: &str) -> io::Result<ShellBytes> {
    let path = record_path(data_dir, tool_id)?;
    match std::fs::read(&path) {
        Ok(bytes) => Ok(decode(&bytes)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            Ok(ShellBytes::Record(ShellRecord::default()))
        }
        Err(err) => Err(err),
    }
}

/// 原子写入。目录不存在时创建。
pub fn write_record(data_dir: &Path, tool_id: &str, record: &ShellRecord) -> io::Result<()> {
    let path = record_path(data_dir, tool_id)?;
    crate::storage::atomic_write(&path, &encode(record))
}

/// 扫到的一份记录。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredShell {
    pub id: String,
    pub record: ShellRecord,
}

/// 扫目录时跳过的文件。文件名去掉 `.json` 之后的 id。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScanSkip {
    Unreadable(String),
    Corrupt(String),
    Unsupported(String),
}

/// 读 `tool-windows/` 里能认的记录。目录不存在时是空。坏文件跳过，不改它们。
pub fn scan_records(data_dir: &Path) -> io::Result<(Vec<StoredShell>, Vec<ScanSkip>)> {
    let dir = data_dir.join(RECORD_DIR);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Ok((Vec::new(), Vec::new()));
        }
        Err(err) => return Err(err),
    };
    let mut stored = Vec::new();
    let mut skipped = Vec::new();
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(id) = name.strip_suffix(".json") else {
            continue;
        };
        if record_relative(id).is_none() {
            continue;
        }
        let bytes = match std::fs::read(entry.path()) {
            Ok(bytes) => bytes,
            Err(_) => {
                skipped.push(ScanSkip::Unreadable(id.to_owned()));
                continue;
            }
        };
        let decoded = decode(&bytes);
        if let Some(record) = decoded.openable() {
            stored.push(StoredShell {
                id: id.to_owned(),
                record,
            });
            continue;
        }
        match decoded {
            ShellBytes::UnsupportedSchema => skipped.push(ScanSkip::Unsupported(id.to_owned())),
            _ => skipped.push(ScanSkip::Corrupt(id.to_owned())),
        }
    }
    stored.sort_by(|left, right| left.id.cmp(&right.id));
    skipped.sort_by(|left, right| skip_id(left).cmp(skip_id(right)));
    Ok((stored, skipped))
}

fn record_path(data_dir: &Path, tool_id: &str) -> io::Result<PathBuf> {
    let relative = record_relative(tool_id)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "工具 id 不能作为文件名"))?;
    Ok(data_dir.join(relative))
}

fn skip_id(skip: &ScanSkip) -> &str {
    match skip {
        ScanSkip::Unreadable(id) | ScanSkip::Corrupt(id) | ScanSkip::Unsupported(id) => id,
    }
}

/// 落盘字节。不选择数据目录里的路径。
#[must_use]
pub fn encode(record: &ShellRecord) -> Vec<u8> {
    let on_disk = OnDisk {
        schema_version: crate::storage::SCHEMA_VERSION,
        geometry: record.geometry,
        dock: record.dock.as_ref(),
        pinned: record.pinned,
        open: record.open,
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
    let open = match obj.get("open") {
        None => false,
        Some(Value::Bool(open)) => *open,
        Some(_) => return ShellBytes::Corrupt,
    };
    let dock = obj.get("floatDock").and_then(FloatDock::from_json);
    let record = ShellRecord {
        geometry: None,
        dock,
        pinned,
        open,
    };
    match obj.get("floatWindow") {
        None => ShellBytes::Record(record),
        Some(value) => match geometry_of(value) {
            Some(geometry) => ShellBytes::Record(ShellRecord {
                geometry: Some(geometry),
                ..record
            }),
            None => ShellBytes::GeometryUnreadable(record),
        },
    }
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

/// 备份用的 JSON。和 [`export_json`] 同一套字段，不含 `open`。
#[must_use]
pub fn backup_json(record: &ShellRecord) -> Vec<u8> {
    export_json(record)
}

/// 放进导出包和备份的窗口记录。落盘格式，去掉 `open`。读不懂时是 `None`。
pub(crate) fn archived_window(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut record = decode(bytes).openable()?;
    record.open = false;
    let encoded = encode(&record);
    let mut value: Value = serde_json::from_slice(encoded.trim_ascii_end()).ok()?;
    value.as_object_mut()?.remove("open");
    let mut out = serde_json::to_vec(&value).ok()?;
    out.push(b'\n');
    Some(out)
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
    open: bool,
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
            open: true,
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
        assert!(raw.contains("\"open\":true"));
        assert!(!raw.contains("floatStack"));

        let cleared = encode(&ShellRecord::default());
        assert_eq!(text(&cleared), "{\"schemaVersion\":1,\"open\":false}\n");
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
    fn export_and_backup_keep_the_same_size_dock_and_pin() {
        let exported = text(&export_json(&full_record()));
        let backed = text(&backup_json(&full_record()));
        assert_eq!(exported, backed);
        assert!(exported.contains("\"x\":12"));
        assert!(exported.contains("\"y\":34"));
        assert!(exported.contains("\"width\":320"));
        assert!(exported.contains("\"height\":360"));
        assert!(exported.contains("\"floatDock\""));
        assert!(exported.contains("\"floatPinned\":true"));
        assert!(!exported.contains("open"));
        assert!(!exported.contains("floatStack"));

        let pin_only = ShellRecord {
            pinned: Some(true),
            ..ShellRecord::default()
        };
        assert_eq!(text(&backup_json(&pin_only)), text(&export_json(&pin_only)));
        let exported = text(&export_json(&pin_only));
        assert!(exported.contains("\"floatPinned\":true"));
        assert!(!exported.contains("width"));
        assert_eq!(text(&backup_json(&ShellRecord::default())), "{}");
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
        assert!(!session.open());
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
        assert!(!session.above_ordinary_windows());

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
            ..ShellRecord::default()
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
            ..ShellRecord::default()
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
        session.set_pinned(false);
        assert_eq!(session.pinned(), Some(false));
        assert!(session.collapsed());
        assert!(!session.above_ordinary_windows());
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
            ..ShellRecord::default()
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
        assert!(!session.above_ordinary_windows());
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
            "沿用工作区函数里已有的最小尺寸"
        );
    }

    #[test]
    fn blur_collapses_a_docked_window_whether_or_not_it_is_pinned() {
        let (display, primary) = bottom_taskbar();
        for pinned in [Some(true), Some(false), None] {
            let mut session = ShellSession::from_record(ShellRecord {
                geometry: Some(FloatGeometry {
                    x: 0,
                    y: 80,
                    width: 320,
                    height: 360,
                }),
                dock: Some(dock(false)),
                pinned,
                open: true,
            });
            let next = session
                .apply(ShellGesture::Blur, &[display], &[primary], primary)
                .unwrap();
            assert!(next.dock.expect("仍贴着").collapsed, "{pinned:?}");
            assert_eq!(session.above_ordinary_windows(), pinned != Some(false));
            assert!(session.open());
        }
    }

    #[test]
    fn an_unplaced_window_uses_the_declared_size_at_the_bottom_right() {
        assert_eq!(shell_logical_size(None), (320, 360));
        assert_eq!(shell_logical_size(Some((400, 200))), (400, 200));
        assert_eq!(shell_logical_size(Some((10, 10))), (200, 120));

        let work = area(0, 0, 1920, 1040);
        assert_eq!(shell_default_origin(work, 320, 360, 96), (1576, 656));
        assert_eq!(shell_default_origin(work, 320, 360, 192), (1552, 632));
        assert_eq!(shell_default_origin(work, 4000, 4000, 96), (0, 0));
        let shifted = area(100, 50, 2020, 1090);
        assert_eq!(
            shell_default_origin(shifted, 320, 360, 96),
            (100 + 1576, 50 + 656)
        );
    }

    #[test]
    fn records_live_under_tool_windows_and_only_open_ones_reopen() {
        assert_eq!(
            record_relative("lanwork.float-shell-demo").unwrap(),
            PathBuf::from("tool-windows/lanwork.float-shell-demo.json")
        );
        assert!(record_relative("../notes").is_none());
        assert!(record_relative("a/b").is_none());
        assert!(record_relative("").is_none());
        let package = include_str!("../backup/package.rs");
        assert!(
            package.contains("Kind::ToolWindow"),
            "窗口记录进入导出白名单"
        );

        let dir = std::env::temp_dir().join(format!(
            "lanwork-shell-{}-{}",
            std::process::id(),
            RECORD_DIR
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let missing = read_record(&dir, "lanwork.float-shell-demo").unwrap();
        assert_eq!(missing, ShellBytes::Record(ShellRecord::default()));

        let mut open = full_record();
        open.open = true;
        write_record(&dir, "lanwork.float-shell-demo", &open).unwrap();
        let mut closed = full_record();
        closed.open = false;
        closed.pinned = Some(false);
        write_record(&dir, "other.tool", &closed).unwrap();
        std::fs::write(dir.join(RECORD_DIR).join("broken.json"), b"{").unwrap();
        std::fs::write(
            dir.join(RECORD_DIR).join("future.json"),
            br#"{"schemaVersion":2,"open":true}"#,
        )
        .unwrap();

        let ShellBytes::Record(read) = read_record(&dir, "lanwork.float-shell-demo").unwrap()
        else {
            panic!("应读回");
        };
        assert_eq!(read, open);
        let (stored, skipped) = scan_records(&dir).unwrap();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].id, "lanwork.float-shell-demo");
        assert!(stored[0].record.open);
        assert!(!stored[1].record.open);
        assert!(skipped.contains(&ScanSkip::Corrupt("broken".to_owned())));
        assert!(skipped.contains(&ScanSkip::Unsupported("future".to_owned())));
        assert!(write_record(&dir, "CON", &open).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreadable_float_window_keeps_the_dock_and_is_not_treated_as_missing() {
        let raw = br#"{"schemaVersion":1,"floatWindow":{"x":"no"},"floatDock":{"edge":"left","offset":80,"collapsed":true},"floatPinned":false,"open":true}"#;
        let decoded = decode(raw);
        assert!(decoded.keep_file_on_open());
        let record = decoded.openable().expect("贴边还能用");
        assert!(record.geometry.is_none());
        assert_eq!(record.dock, Some(dock(true)));
        assert_eq!(record.pinned, Some(false));
        assert!(record.open);

        let missing = decode(
            br#"{"schemaVersion":1,"floatDock":{"edge":"left","offset":80,"collapsed":false},"open":true}"#,
        );
        assert!(!missing.keep_file_on_open(), "字段不在时仍按没记过位置");
        assert!(matches!(missing, ShellBytes::Record(_)));

        assert_eq!(
            decode(br#"{"schemaVersion":1,"floatWindow":"no","floatPinned":"yes"}"#),
            ShellBytes::Corrupt,
            "钉住不是布尔时整份仍拒绝"
        );

        let (display, primary) = bottom_taskbar();
        let size = (320u32, 360u32);
        let anchored = geometry_for_open(&record, true, size, display).expect("贴在左边");
        let kept = settle_float(
            DockState {
                expanded: PxBox {
                    x: anchored.x,
                    y: anchored.y,
                    width: anchored.width,
                    height: anchored.height,
                },
                dock: record.dock,
            },
            &[display],
            &[primary],
            primary,
        );
        assert_eq!(kept.dock, record.dock, "合成矩形贴在外边上，贴边留下");

        let (x, y) = shell_default_origin(primary.area, 320, 360, 96);
        let dropped = settle_float(
            DockState {
                expanded: PxBox {
                    x,
                    y,
                    width: 320,
                    height: 360,
                },
                dock: record.dock,
            },
            &[display],
            &[primary],
            primary,
        );
        assert!(
            dropped.dock.is_none(),
            "没有几何时若当成缺字段，默认右下角会丢掉左边贴边"
        );

        let right = FloatDock {
            edge: DockEdge::Right,
            offset: 40,
            collapsed: false,
        };
        let top = FloatDock {
            edge: DockEdge::Top,
            offset: 50,
            collapsed: false,
        };
        for dock in [right, top] {
            let record = ShellRecord {
                dock: Some(dock),
                open: true,
                ..ShellRecord::default()
            };
            let geometry = geometry_for_open(&record, true, size, display).unwrap();
            let settled = settle_float(
                DockState {
                    expanded: PxBox {
                        x: geometry.x,
                        y: geometry.y,
                        width: geometry.width,
                        height: geometry.height,
                    },
                    dock: Some(dock),
                },
                &[display],
                &[primary],
                primary,
            );
            assert_eq!(settled.dock, Some(dock));
        }

        let no_dock = ShellRecord {
            open: true,
            ..ShellRecord::default()
        };
        assert!(geometry_for_open(&no_dock, true, size, display).is_none());
    }

    #[test]
    fn unreadable_geometry_is_scanned_for_reopen_and_a_failed_write_keeps_the_file() {
        let dir = std::env::temp_dir().join(format!(
            "lanwork-shell-bad-geo-{}-{}",
            std::process::id(),
            RECORD_DIR
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let id = "lanwork.float-shell-demo";
        let path = dir.join(record_relative(id).unwrap());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = br#"{"schemaVersion":1,"floatWindow":{"x":1,"y":2},"floatDock":{"edge":"right","offset":12,"collapsed":false},"open":true}"#;
        std::fs::write(&path, original).unwrap();

        let read = read_record(&dir, id).unwrap();
        assert!(read.keep_file_on_open());
        let record = read.openable().unwrap();
        assert_eq!(record.dock.unwrap().edge, DockEdge::Right);
        let (stored, skipped) = scan_records(&dir).unwrap();
        assert_eq!(stored.len(), 1);
        assert!(stored[0].record.open);
        assert!(stored[0].record.dock.is_some());
        assert!(skipped.is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), original);

        let err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "磁盘");
        assert_eq!(write_failure_text(&err), "保存失败：磁盘");

        lock_record(&path);
        let write_err = write_record(&dir, id, &full_record());
        unlock_record(&path);
        let write_err = write_err.expect_err("占用或只读时替换失败");
        assert!(write_failure_text(&write_err).starts_with("保存失败："));
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(!path.with_extension("json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    fn lock_record(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let dir = path.parent().unwrap();
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    }

    #[cfg(unix)]
    fn unlock_record(path: &std::path::Path) {
        use std::os::unix::fs::PermissionsExt;
        let dir = path.parent().unwrap();
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(windows)]
    fn record_lock() -> &'static std::sync::Mutex<Option<std::fs::File>> {
        static LOCK: std::sync::Mutex<Option<std::fs::File>> = std::sync::Mutex::new(None);
        &LOCK
    }

    #[cfg(windows)]
    fn lock_record(path: &std::path::Path) {
        use std::fs::OpenOptions;
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ = 1。不带 FILE_SHARE_DELETE，替换必须失败，原文件留下。
        let file = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(path)
            .unwrap();
        *record_lock().lock().unwrap() = Some(file);
    }

    #[cfg(windows)]
    fn unlock_record(_path: &std::path::Path) {
        *record_lock().lock().unwrap() = None;
    }
}
