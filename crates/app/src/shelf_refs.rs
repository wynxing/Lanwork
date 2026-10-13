//! 收纳页和收纳悬浮窗共用的条目列表逻辑：行数据、可见行的存在检查与图标、按下与拖出、放下的路径整理。
//!
//! 两处各有自己的窗口和 `ShelvesPageData` 全局（全局在每个窗口里各一份），这里只持有状态，
//! 不决定把结果交给哪个窗口。路径引用不打开、不复制、不移动、不删除原文件。网页引用只保存网址，
//! 不读取虚拟 `.url` 的内容，也不关闭或移动浏览器标签。网页行不拖出。

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use lanwork_core::apps::{LaunchTarget, launch};
use lanwork_core::dispatch::{IconCache, IconKey, RgbaImage};
use lanwork_core::panel::{LinkedTodo, ShelfLine, passed_drag_threshold};
use lanwork_core::shell::path_target;
use lanwork_core::shelves::{IncomingRef, ShelfCommands};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{LogicalPosition, Model, ModelRc, VecModel};

use crate::shelf_dnd::{DragOutcome, drag_metrics, drag_out, path_text};
use crate::{ShelfLinkRow, ShelfRefRow, ShelvesPageData};

/// 按下条目行时记下的位置，指针移出阈值之后才开始拖出。
struct Press {
    path: String,
    x: f32,
    y: f32,
}

/// 一次后台检查：哪些路径要查是否存在，哪些要取图标。
pub(crate) struct ProbePlan {
    epoch: u64,
    to_check: Vec<String>,
    to_icon: Vec<String>,
}

/// 后台检查的结果。
pub(crate) struct ProbeResult {
    pub(crate) epoch: u64,
    exists: Vec<(String, bool)>,
    icons: Vec<(String, RgbaImage)>,
}

/// 指针移动之后要做什么。
pub(crate) enum Moved {
    Nothing,
    /// 超过阈值但已知路径不存在。
    Missing,
    /// 超过阈值，开始拖出这一条。`origin` 是按下的位置。
    Start {
        path: String,
        origin: (f32, f32),
    },
}

pub(crate) struct RefsView {
    model: Rc<VecModel<ShelfRefRow>>,
    exists: HashMap<String, bool>,
    icons: HashMap<String, slint::Image>,
    epoch: u64,
    first: usize,
    count: usize,
    selected: Option<String>,
    /// 选中的是网页引用。路径和网址字符串碰巧相同也不能当成同一条。
    selected_web: bool,
    press: Option<Press>,
}

impl RefsView {
    pub(crate) fn new() -> Self {
        Self {
            model: Rc::default(),
            exists: HashMap::new(),
            icons: HashMap::new(),
            epoch: 0,
            first: 0,
            count: 0,
            selected: None,
            selected_web: false,
            press: None,
        }
    }

    pub(crate) fn model(&self) -> Rc<VecModel<ShelfRefRow>> {
        Rc::clone(&self.model)
    }

    pub(crate) fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    pub(crate) fn selected_is_web(&self) -> bool {
        self.selected_web
    }

    /// 换了分组：忘掉选中、按下和已知的存在、图标状态。
    pub(crate) fn clear(&mut self) {
        self.selected = None;
        self.selected_web = false;
        self.press = None;
        self.exists.clear();
        self.icons.clear();
    }

    pub(crate) fn set_visible(&mut self, first: i32, count: i32) {
        self.first = usize::try_from(first).unwrap_or(0);
        self.count = usize::try_from(count).unwrap_or(0);
    }

    pub(crate) fn is_missing(&self, path: &str) -> bool {
        self.exists.get(path) == Some(&false)
    }

    pub(crate) fn select(&mut self, lines: &[ShelfLine], index: i32) {
        if let Some(line) = usize::try_from(index)
            .ok()
            .and_then(|index| lines.get(index))
        {
            self.selected = Some(line.key.clone());
            self.selected_web = line.web;
        }
    }

    pub(crate) fn deselect(&mut self) {
        self.selected = None;
        self.selected_web = false;
    }

    /// 选中的条目已经不在分组里时取消选中。
    pub(crate) fn keep_selected(&mut self, lines: &[ShelfLine]) {
        let still = self.selected.as_ref().is_some_and(|key| {
            lines
                .iter()
                .any(|line| line.web == self.selected_web && &line.key == key)
        });
        if !still {
            self.selected = None;
            self.selected_web = false;
        }
    }

    fn same_line(&self, line: &ShelfLine) -> bool {
        self.selected.as_deref() == Some(line.key.as_str()) && self.selected_web == line.web
    }

    /// 条目行跟着分组和已知的存在、图标状态重排。行数不变时逐行更新，列表的滚动位置不动。
    /// 网页行不取图标，也不标「路径不存在」。
    pub(crate) fn push(&self, lines: &[ShelfLine], data: &ShelvesPageData<'_>) {
        let rows: Vec<ShelfRefRow> = lines
            .iter()
            .map(|line| {
                let icon = if line.web {
                    None
                } else {
                    self.icons.get(&line.key)
                };
                ShelfRefRow {
                    name: line.name.as_str().into(),
                    icon: icon.cloned().unwrap_or_default(),
                    has_icon: icon.is_some(),
                    missing: !line.web && self.is_missing(&line.key),
                }
            })
            .collect();
        if self.model.row_count() == rows.len() {
            for (index, row) in rows.into_iter().enumerate() {
                self.model.set_row_data(index, row);
            }
        } else {
            self.model.set_vec(rows);
        }
        let index = lines
            .iter()
            .position(|line| self.same_line(line))
            .map_or(-1, to_i32);
        data.set_selected_ref(index);
        data.set_selected_path(self.selected.clone().unwrap_or_default().into());
    }

    /// 只给可见的路径行查是否存在和取图标。网页行跳过。`force` 为真时丢掉已知的存在状态重新查。
    pub(crate) fn refresh(
        &mut self,
        lines: &[ShelfLine],
        force: bool,
        data: &ShelvesPageData<'_>,
    ) -> Option<ProbePlan> {
        let visible: Vec<&ShelfLine> = lines
            .iter()
            .skip(self.first)
            .take(self.count.max(1))
            .filter(|line| !line.web)
            .collect();
        if force {
            self.epoch = self.epoch.wrapping_add(1);
            for line in &visible {
                self.exists.remove(&line.key);
            }
        }
        let to_check: Vec<String> = visible
            .iter()
            .filter(|line| !self.exists.contains_key(&line.key))
            .map(|line| line.key.clone())
            .collect();
        let to_icon: Vec<String> = visible
            .iter()
            .filter(|line| !self.icons.contains_key(&line.key))
            .map(|line| line.key.clone())
            .collect();
        let keep: HashSet<&str> = visible.iter().map(|line| line.key.as_str()).collect();
        let before = self.icons.len();
        self.icons.retain(|path, _| keep.contains(path.as_str()));
        if self.icons.len() != before {
            self.push(lines, data);
        }
        (!to_check.is_empty() || !to_icon.is_empty()).then_some(ProbePlan {
            epoch: self.epoch,
            to_check,
            to_icon,
        })
    }

    /// 后台检查的结果。旧轮次的存在结果只补空缺，不覆盖新的。
    pub(crate) fn receive(&mut self, result: ProbeResult) {
        if result.epoch == self.epoch {
            self.exists.extend(result.exists);
        } else {
            for (path, present) in result.exists {
                self.exists.entry(path).or_insert(present);
            }
        }
        for (path, image) in &result.icons {
            self.icons.insert(path.clone(), to_image(image));
        }
    }

    pub(crate) fn press(&mut self, lines: &[ShelfLine], index: i32, x: f32, y: f32) {
        let line = usize::try_from(index)
            .ok()
            .and_then(|index| lines.get(index));
        self.press = line.filter(|line| !line.web).map(|line| Press {
            path: line.key.clone(),
            x,
            y,
        });
    }

    pub(crate) fn release(&mut self) {
        self.press = None;
    }

    /// 移动超过系统拖动阈值（物理像素）就拖出按下的那一条。
    pub(crate) fn moved(&mut self, scale: f32, x: f32, y: f32) -> Moved {
        let Some(press) = self.press.as_ref() else {
            return Moved::Nothing;
        };
        let (cx, cy) = drag_metrics();
        if !passed_drag_threshold((x - press.x) * scale, (y - press.y) * scale, cx, cy) {
            return Moved::Nothing;
        }
        let Some(press) = self.press.take() else {
            return Moved::Nothing;
        };
        if self.is_missing(&press.path) {
            return Moved::Missing;
        }
        Moved::Start {
            origin: (press.x, press.y),
            path: press.path,
        }
    }
}

/// 在后台线程检查路径是否存在、取图标，完成后在工作线程里调用 `deliver`。
/// 图标来自共享缓存；已知不存在的路径不取图。
pub(crate) fn spawn_probe(
    shelves: ShelfCommands,
    cache: Arc<Mutex<IconCache>>,
    plan: ProbePlan,
    deliver: impl FnOnce(ProbeResult) + Send + 'static,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("lanwork-shelf-probe".into())
        .spawn(move || {
            let flags = shelves.check_exists(&plan.to_check);
            let exists: Vec<(String, bool)> = plan.to_check.into_iter().zip(flags).collect();
            let missing: HashSet<&String> = exists
                .iter()
                .filter(|(_, present)| !present)
                .map(|(path, _)| path)
                .collect();
            let mut icons = Vec::new();
            for path in plan.to_icon {
                if missing.contains(&path) {
                    continue;
                }
                let key = IconKey {
                    path: path.clone(),
                    index: 0,
                };
                let image = cache
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(&key);
                if let Some(image) = image {
                    icons.push((path, image));
                }
            }
            deliver(ProbeResult {
                epoch: plan.epoch,
                exists,
                icons,
            });
        })
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// 拖动阈值触发的那次回调里，同步进入 OLE 的拖出循环所需的交接。
///
/// `DoDragDrop` 是模态循环，不能在窗口的 `RefCell` 借用里跑；但推迟到下一轮事件循环时，鼠标抬起可能
/// 已经入队，循环一开始就看到按钮已松开而立刻结束。所以回调先在借用里只算出要拖的路径并交给
/// [`InlineDrag::offer`]，借用结束后再由 [`InlineDrag::take`] 取走，在同一次回调里开始拖出。
/// 如果回调本身就落在别的借用里（动作被推迟），`offer` 返回 false，退回 [`schedule_drag`]。
pub(crate) struct InlineDrag {
    open: Cell<bool>,
    start: RefCell<Option<(String, (f32, f32))>>,
}

impl InlineDrag {
    pub(crate) fn new() -> Rc<Self> {
        Rc::new(Self {
            open: Cell::new(true),
            start: RefCell::new(None),
        })
    }

    /// 动作在回调里同步执行时接收要拖的路径。动作被推迟之后返回 false。
    pub(crate) fn offer(&self, path: String, origin: (f32, f32)) -> bool {
        if !self.open.get() {
            return false;
        }
        *self.start.borrow_mut() = Some((path, origin));
        true
    }

    /// 借用结束后取走同步接收到的路径，之后的 `offer` 都返回 false。
    pub(crate) fn take(&self) -> Option<(String, (f32, f32))> {
        self.open.set(false);
        self.start.borrow_mut().take()
    }
}

/// 立刻进入 OLE 的拖出循环。调用者不能持有窗口的借用：循环期间窗口消息照常派发。
/// `before` 在进入循环前调用，`after` 在循环结束后调用。
pub(crate) fn drag_now(
    path: String,
    before: impl FnOnce(),
    after: impl FnOnce(Result<DragOutcome, String>),
) {
    before();
    let outcome = drag_out(&[PathBuf::from(path)]);
    after(outcome);
}

/// 动作被推迟（回调落在别的借用里）时的退路：下一轮事件循环再拖出。
pub(crate) fn schedule_drag(
    path: String,
    before: impl FnOnce() + 'static,
    after: impl FnOnce(Result<DragOutcome, String>) + 'static,
) {
    slint::Timer::single_shot(Duration::ZERO, move || drag_now(path, before, after));
}

/// 模态循环吃掉了鼠标抬起，行上的 `TouchArea` 还抓着鼠标。补发抬起和离开，抓取才会释放。
pub(crate) fn release_grab(window: &slint::Window, origin: (f32, f32)) {
    window.dispatch_event(WindowEvent::PointerReleased {
        position: LogicalPosition::new(origin.0, origin.1),
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerExited);
}

/// 拖出结束后要显示的错误。取消和放下都不是错误。
pub(crate) fn drag_error(outcome: Result<DragOutcome, String>) -> Option<String> {
    match outcome {
        Ok(outcome) if outcome.failed() => Some(format!("拖出失败：{}", outcome.result.message())),
        Ok(_) => None,
        Err(message) => Some(format!("拖出失败：{message}")),
    }
}

/// OLE 放下目标读到路径之后调用。文件夹标记在后台线程里查，完成后在工作线程里调用 `then`
/// （路径列表和被跳过的不合法 Unicode 路径个数）。
pub(crate) fn prepare_drop(
    paths: Vec<PathBuf>,
    then: impl FnOnce(Vec<IncomingRef>, usize) + Send + 'static,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("lanwork-shelf-drop".into())
        .spawn(move || {
            let mut incoming = Vec::with_capacity(paths.len());
            let mut skipped = 0;
            for path in paths {
                match path_text(&path) {
                    Some(text) => incoming.push(IncomingRef {
                        folder: std::fs::metadata(&path).is_ok_and(|meta| meta.is_dir()),
                        path: text,
                    }),
                    None => skipped += 1,
                }
            }
            then(incoming, skipped);
        })
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// 路径的上一级，用于 Ctrl+Enter 打开所在目录。
pub(crate) fn parent_folder(path: &str) -> Option<String> {
    Path::new(path)
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .and_then(Path::to_str)
        .map(str::to_owned)
}

/// 用默认浏览器打开保存的网址。放在后台线程，失败时在工作线程里调用 `on_error`。
pub(crate) fn spawn_open_url(
    url: String,
    on_error: impl FnOnce(String) + Send + 'static,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("lanwork-shelf-open".into())
        .spawn(move || {
            if let Err(err) = launch(&LaunchTarget::Url { url }) {
                on_error(err.message);
            }
        })
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// 用 Windows Shell 打开路径。放在后台线程，失败时在工作线程里调用 `on_error`。
pub(crate) fn spawn_launch(
    path: String,
    on_error: impl FnOnce(String) + Send + 'static,
) -> Result<(), String> {
    std::thread::Builder::new()
        .name("lanwork-shelf-open".into())
        .spawn(move || {
            if let Err(err) = launch(&path_target(&path)) {
                on_error(err.message);
            }
        })
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// 加入失败之外的提示：有路径不是合法 Unicode 而被跳过。
pub(crate) fn skipped_message(skipped: usize) -> Option<String> {
    (skipped > 0).then(|| format!("{skipped} 个路径不是有效的 Unicode，没有加入"))
}

pub(crate) fn to_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn to_image(image: &RgbaImage) -> slint::Image {
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        &image.pixels,
        image.width,
        image.height,
    );
    slint::Image::from_rgba8(buffer)
}

/// 把选中分组的关联待办推给界面。面板和悬浮窗各有一份 `ShelvesPageData`，都调这个。
pub(crate) fn push_links(links: &[LinkedTodo], data: &ShelvesPageData<'_>) {
    let rows: Vec<ShelfLinkRow> = links
        .iter()
        .map(|link| ShelfLinkRow {
            title: link.title.as_str().into(),
            found: link.found,
            completed: link.completed,
            in_trash: link.in_trash,
        })
        .collect();
    data.set_links(ModelRc::new(VecModel::from(rows)));
}

/// 多条关联待办在分组列表里的一行文字：标题用「、」连起来。怎么排版产品规格没有写，这是实现选择。
pub(crate) fn joined_titles(titles: &[String]) -> String {
    titles.join("、")
}

#[cfg(test)]
mod tests {
    use windows::Win32::Foundation::{DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, E_FAIL};

    use super::*;

    #[test]
    fn linked_titles_are_joined_with_the_enumeration_comma() {
        assert_eq!(joined_titles(&[]), "");
        assert_eq!(joined_titles(&["甲".to_owned()]), "甲");
        assert_eq!(joined_titles(&["甲".to_owned(), "乙".to_owned()]), "甲、乙");
    }

    #[test]
    fn an_inline_drag_is_handed_over_once_and_a_deferred_action_is_refused() {
        let inline = InlineDrag::new();
        assert!(inline.offer("C:\\a.txt".into(), (1.0, 2.0)));
        assert_eq!(inline.take(), Some(("C:\\a.txt".to_owned(), (1.0, 2.0))));
        assert_eq!(inline.take(), None);
        assert!(!inline.offer("C:\\b.txt".into(), (0.0, 0.0)));
        assert_eq!(inline.take(), None);
    }

    #[test]
    fn the_folder_to_open_is_the_parent_and_a_root_has_none() {
        assert_eq!(
            parent_folder(r"C:\Users\me\a.txt").as_deref(),
            Some(r"C:\Users\me")
        );
        assert_eq!(parent_folder(r"C:\Users\me").as_deref(), Some(r"C:\Users"));
        assert_eq!(parent_folder(r"C:\"), None);
        assert_eq!(parent_folder(""), None);
    }

    #[test]
    fn dropping_or_cancelling_a_drag_is_not_an_error_but_a_failure_is() {
        let outcome = |result| Ok(DragOutcome { result });
        assert_eq!(drag_error(outcome(DRAGDROP_S_DROP)), None);
        assert_eq!(drag_error(outcome(DRAGDROP_S_CANCEL)), None);
        assert!(drag_error(outcome(E_FAIL)).is_some());
        assert!(
            drag_error(Err("SHParseDisplayName".to_owned()))
                .is_some_and(|text| text.starts_with("拖出失败"))
        );
    }

    #[test]
    fn skipped_paths_are_reported_only_when_there_are_some() {
        assert_eq!(skipped_message(0), None);
        assert_eq!(
            skipped_message(2).as_deref(),
            Some("2 个路径不是有效的 Unicode，没有加入")
        );
    }

    #[test]
    fn a_drop_marks_folders_and_leaves_the_files_alone() {
        use std::sync::mpsc;
        let root = std::env::temp_dir().join(format!("lanwork-shelf-refs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("folder")).expect("folder");
        std::fs::write(root.join("a.txt"), b"a").expect("file");
        let (send, received) = mpsc::channel();
        prepare_drop(
            vec![root.join("a.txt"), root.join("folder"), root.join("gone")],
            move |incoming, skipped| {
                let _ = send.send((incoming, skipped));
            },
        )
        .expect("worker");
        let (incoming, skipped) = received
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("result");
        assert_eq!(skipped, 0);
        let folder_flags: Vec<bool> = incoming.iter().map(|item| item.folder).collect();
        assert_eq!(folder_flags, [false, true, false]);
        assert_eq!(std::fs::read(root.join("a.txt")).expect("kept"), b"a");
        let _ = std::fs::remove_dir_all(&root);
    }
}
