//! 面板的收纳页：分组列表、条目、关联待办，以及拖入和拖出。
//!
//! 窗口在 `panel`，版式在 `ui/shelvespage.slint`，OLE 在 `shelf_dnd`，规则在 `lanwork_core::shelves`
//! 和 `lanwork_core::panel`。这里把服务的结果排成界面要的行，并把用户的动作交给服务。
//! 只保存路径引用；不打开、不复制、不移动、不删除被引用的文件。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use lanwork_core::apps::launch;
use lanwork_core::dispatch::{IconCache, IconKey, RgbaImage};
use lanwork_core::panel::{
    GroupRow, TodoChoice, first_group_for_todo, group_rows, linked_todo_ids, passed_drag_threshold,
    pick_selection, reordered_group_ids, todo_choices, visible_paths,
};
use lanwork_core::shell::path_target;
use lanwork_core::shelves::{IncomingRef, Shelf, ShelfCommands};
use lanwork_core::storage::Store;
use lanwork_core::todos::{TodoCommands, TodoList};
use slint::platform::{PointerEventButton, WindowEvent};
use slint::{ComponentHandle, LogicalPosition, Model, ModelRc, SharedString, VecModel};

use crate::panel::with_shelves;
use crate::shelf_dnd::{DragOutcome, drag_metrics, drag_out, path_text};
use crate::{Panel, ShelfGroupRow, ShelfRefRow, ShelfTodoRow, ShelvesPageData};

/// 按下条目行时记下的位置，指针移出阈值之后才开始拖出。
struct Press {
    path: String,
    x: f32,
    y: f32,
}

pub(crate) struct ShelvesPage {
    shelves: Option<ShelfCommands>,
    todos: TodoCommands,
    icon_cache: Arc<Mutex<IconCache>>,
    store: Store,
    groups_model: Rc<VecModel<ShelfGroupRow>>,
    refs_model: Rc<VecModel<ShelfRefRow>>,
    choices_model: Rc<VecModel<ShelfTodoRow>>,
    list: Vec<Shelf>,
    lists: Vec<TodoList>,
    rows: Vec<GroupRow>,
    selected: Option<String>,
    shown_group: Option<String>,
    selected_ref: Option<String>,
    exists: HashMap<String, bool>,
    icons: HashMap<String, slint::Image>,
    epoch: u64,
    first_visible: usize,
    visible_count: usize,
    choices: Vec<TodoChoice>,
    press: Option<Press>,
    dragging: bool,
}

impl ShelvesPage {
    pub(crate) fn new(
        ui: &Panel,
        shelves: Option<ShelfCommands>,
        todos: TodoCommands,
        icon_cache: Arc<Mutex<IconCache>>,
        store: Store,
    ) -> Self {
        let page = Self {
            shelves,
            todos,
            icon_cache,
            store,
            groups_model: Rc::default(),
            refs_model: Rc::default(),
            choices_model: Rc::default(),
            list: Vec::new(),
            lists: Vec::new(),
            rows: Vec::new(),
            selected: None,
            shown_group: None,
            selected_ref: None,
            exists: HashMap::new(),
            icons: HashMap::new(),
            epoch: 0,
            first_visible: 0,
            visible_count: 0,
            choices: Vec::new(),
            press: None,
            dragging: false,
        };
        let data = ui.global::<ShelvesPageData>();
        data.set_groups(ModelRc::from(Rc::clone(&page.groups_model)));
        data.set_refs(ModelRc::from(Rc::clone(&page.refs_model)));
        data.set_choices(ModelRc::from(Rc::clone(&page.choices_model)));
        data.set_selected_group(-1);
        data.set_selected_ref(-1);
        wire(ui);
        page
    }

    /// 拖出的模态循环正在进行。面板此时不因失焦收起。
    pub(crate) fn is_dragging(&self) -> bool {
        self.dragging
    }

    /// 被至少一个分组关联的待办，待办页据此显示标记。服务没有打开时为空。
    pub(crate) fn linked_todos(&self) -> HashSet<String> {
        self.shelves
            .as_ref()
            .and_then(|shelves| shelves.list().ok())
            .map(|list| linked_todo_ids(&list))
            .unwrap_or_default()
    }

    /// 关联了这条待办的分组里排在最前的一个。
    pub(crate) fn group_for_todo(&self, todo_id: &str) -> Option<String> {
        let list = self.shelves.as_ref()?.list().ok()?;
        first_group_for_todo(&list, todo_id).map(str::to_owned)
    }

    /// 切到收纳标签，或面板在收纳标签上显示时调用。
    pub(crate) fn entered(&mut self, ui: &Panel) {
        ui.set_error(SharedString::new());
        self.reload(ui);
    }

    /// 选中某个分组。找不到时显示错误。
    pub(crate) fn locate(&mut self, ui: &Panel, group_id: &str) {
        self.reload(ui);
        if self.list.iter().any(|shelf| shelf.id == group_id) {
            self.selected = Some(group_id.to_owned());
            self.rebuild(ui);
        } else {
            self.fail(ui, "找不到收纳分组");
        }
    }

    fn fail(&self, ui: &Panel, message: &str) {
        self.store.log_warn(&format!("收纳页：{message}"));
        ui.set_error(message.into());
    }

    /// 从服务重读分组和待办，保持选中的分组，然后推给界面。
    pub(crate) fn reload(&mut self, ui: &Panel) {
        let Some(shelves) = self.shelves.clone() else {
            ui.set_error("收纳服务没有打开".into());
            return;
        };
        match shelves.list() {
            Ok(list) => self.list = list,
            Err(err) => {
                self.fail(ui, &err.to_string());
                return;
            }
        }
        match self.todos.lists() {
            Ok(lists) => self.lists = lists,
            Err(err) => {
                self.store
                    .log_warn(&format!("收纳页读不到待办，分组不显示待办标题：{err}"));
                self.lists = Vec::new();
            }
        }
        self.selected = pick_selection(&self.list, self.selected.as_deref());
        self.rebuild(ui);
    }

    fn selected_shelf(&self) -> Option<&Shelf> {
        let id = self.selected.as_ref()?;
        self.list.iter().find(|shelf| &shelf.id == id)
    }

    fn selected_index(&self) -> i32 {
        self.selected
            .as_ref()
            .and_then(|id| self.list.iter().position(|shelf| &shelf.id == id))
            .map_or(-1, count)
    }

    fn rebuild(&mut self, ui: &Panel) {
        self.rows = group_rows(&self.list, &self.lists);
        self.groups_model.set_vec(
            self.rows
                .iter()
                .map(|row| ShelfGroupRow {
                    name: row.name.as_str().into(),
                    count: count(row.count),
                    todo: row.todo_title.as_str().into(),
                })
                .collect::<Vec<_>>(),
        );
        let data = ui.global::<ShelvesPageData>();
        data.set_selected_group(self.selected_index());

        let changed = self.shown_group != self.selected;
        if changed {
            self.shown_group.clone_from(&self.selected);
            self.selected_ref = None;
            self.press = None;
            data.set_confirm_delete(false);
            data.set_picker_open(false);
            data.set_group_version(data.get_group_version().wrapping_add(1));
        }
        let shelf = self.selected_shelf();
        data.set_group_name(
            shelf
                .map(|shelf| shelf.name.as_str())
                .unwrap_or_default()
                .into(),
        );
        data.set_linked(shelf.is_some_and(|shelf| shelf.todo_id.is_some()));
        let todo_title = self
            .selected
            .as_ref()
            .and_then(|id| self.rows.iter().find(|row| &row.id == id))
            .map(|row| row.todo_title.clone())
            .unwrap_or_default();
        data.set_linked_todo(todo_title.into());
        let (can_up, can_down) = match &self.selected {
            Some(id) => (
                reordered_group_ids(&self.list, id, true).is_some(),
                reordered_group_ids(&self.list, id, false).is_some(),
            ),
            None => (false, false),
        };
        data.set_can_up(can_up);
        data.set_can_down(can_down);

        if let Some(path) = &self.selected_ref
            && !self
                .selected_shelf()
                .is_some_and(|shelf| shelf.refs.iter().any(|item| &item.path == path))
        {
            self.selected_ref = None;
        }
        if changed {
            self.exists.clear();
            self.icons.clear();
        }
        self.push_refs(ui);
        self.refresh_visible(ui, true);
    }

    /// 条目行跟着当前分组和已知的存在、图标状态重排。行数不变时逐行更新，列表的滚动位置不动。
    fn push_refs(&self, ui: &Panel) {
        let rows: Vec<ShelfRefRow> = self
            .selected_shelf()
            .map(|shelf| {
                shelf
                    .refs
                    .iter()
                    .map(|item| {
                        let icon = self.icons.get(&item.path);
                        ShelfRefRow {
                            name: item.name.as_str().into(),
                            icon: icon.cloned().unwrap_or_default(),
                            has_icon: icon.is_some(),
                            missing: self.exists.get(&item.path) == Some(&false),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        if self.refs_model.row_count() == rows.len() {
            for (index, row) in rows.into_iter().enumerate() {
                self.refs_model.set_row_data(index, row);
            }
        } else {
            self.refs_model.set_vec(rows);
        }
        let data = ui.global::<ShelvesPageData>();
        let (index, path) = match (&self.selected_ref, self.selected_shelf()) {
            (Some(path), Some(shelf)) => (
                shelf
                    .refs
                    .iter()
                    .position(|item| &item.path == path)
                    .map_or(-1, count),
                path.clone(),
            ),
            _ => (-1, String::new()),
        };
        data.set_selected_ref(index);
        data.set_selected_path(path.into());
    }

    /// 只给可见的行查路径是否存在和取图标。查询放在后台线程，结果回到界面线程再合并。
    /// `force` 为真时丢掉已知的存在状态重新查，这样文件被删掉或补回之后能更新。
    fn refresh_visible(&mut self, ui: &Panel, force: bool) {
        let (Some(shelves), Some(shelf)) = (self.shelves.clone(), self.selected_shelf()) else {
            return;
        };
        let group_id = shelf.id.clone();
        let visible = visible_paths(&shelf.refs, self.first_visible, self.visible_count.max(1));
        if force {
            self.epoch = self.epoch.wrapping_add(1);
            for path in &visible {
                self.exists.remove(path);
            }
        }
        let to_check: Vec<String> = visible
            .iter()
            .filter(|path| !self.exists.contains_key(*path))
            .cloned()
            .collect();
        let to_icon: Vec<String> = visible
            .iter()
            .filter(|path| !self.icons.contains_key(*path))
            .cloned()
            .collect();
        let keep: HashSet<&String> = visible.iter().collect();
        let before = self.icons.len();
        self.icons.retain(|path, _| keep.contains(path));
        if self.icons.len() != before {
            self.push_refs(ui);
        }
        if to_check.is_empty() && to_icon.is_empty() {
            return;
        }
        let epoch = self.epoch;
        let cache = Arc::clone(&self.icon_cache);
        let spawned = std::thread::Builder::new()
            .name("lanwork-shelf-probe".into())
            .spawn(move || {
                let flags = shelves.check_exists(&to_check);
                let exists: Vec<(String, bool)> = to_check.into_iter().zip(flags).collect();
                let known_missing: HashSet<&String> = exists
                    .iter()
                    .filter(|(_, present)| !present)
                    .map(|(path, _)| path)
                    .collect();
                let mut icons = Vec::new();
                for path in to_icon {
                    if known_missing.contains(&path) {
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
                let _ = slint::invoke_from_event_loop(move || {
                    with_shelves(move |page, ui| {
                        page.receive(ui, epoch, &group_id, exists, &icons);
                    });
                });
            });
        if let Err(err) = spawned {
            self.store
                .log_warn(&format!("收纳页没有启动路径检查线程：{err}"));
        }
    }

    fn receive(
        &mut self,
        ui: &Panel,
        epoch: u64,
        group_id: &str,
        exists: Vec<(String, bool)>,
        icons: &[(String, RgbaImage)],
    ) {
        if self.selected.as_deref() != Some(group_id) {
            return;
        }
        if epoch == self.epoch {
            self.exists.extend(exists);
        } else {
            for (path, present) in exists {
                self.exists.entry(path).or_insert(present);
            }
        }
        for (path, image) in icons {
            self.icons.insert(path.clone(), to_image(image));
        }
        self.push_refs(ui);
    }

    fn select_group(&mut self, ui: &Panel, index: i32) {
        let id = usize::try_from(index)
            .ok()
            .and_then(|index| self.rows.get(index))
            .map(|row| row.id.clone());
        let Some(id) = id else {
            return;
        };
        if self.selected.as_deref() == Some(id.as_str()) {
            return;
        }
        ui.set_error(SharedString::new());
        self.selected = Some(id);
        self.rebuild(ui);
    }

    fn new_group(&mut self, ui: &Panel, name: &str) {
        let Some(shelves) = self.shelves.clone() else {
            return;
        };
        match shelves.create_group(name) {
            Ok(shelf) => {
                ui.set_error(SharedString::new());
                self.selected = Some(shelf.id);
                self.reload(ui);
            }
            Err(err) => self.fail(ui, &err.to_string()),
        }
    }

    fn rename_group(&mut self, ui: &Panel, name: &str) {
        let (Some(shelves), Some(id)) = (self.shelves.clone(), self.selected.clone()) else {
            return;
        };
        match shelves.rename_group(&id, name) {
            Ok(_) => {
                ui.set_error(SharedString::new());
                self.reload(ui);
            }
            Err(err) => {
                self.fail(ui, &err.to_string());
                let data = ui.global::<ShelvesPageData>();
                data.set_group_version(data.get_group_version().wrapping_add(1));
            }
        }
    }

    fn move_group(&mut self, ui: &Panel, up: bool) {
        let (Some(shelves), Some(id)) = (self.shelves.clone(), self.selected.clone()) else {
            return;
        };
        let Some(ids) = reordered_group_ids(&self.list, &id, up) else {
            return;
        };
        match shelves.reorder_groups(&ids) {
            Ok(()) => {
                ui.set_error(SharedString::new());
                self.reload(ui);
            }
            Err(err) => self.fail(ui, &err.to_string()),
        }
    }

    fn delete_group(&mut self, ui: &Panel) {
        let (Some(shelves), Some(id)) = (self.shelves.clone(), self.selected.clone()) else {
            return;
        };
        match shelves.delete_group(&id) {
            Ok(()) => {
                ui.set_error(SharedString::new());
                self.reload(ui);
            }
            Err(err) => self.fail(ui, &err.to_string()),
        }
    }

    fn ref_path(&self, index: i32) -> Option<String> {
        let index = usize::try_from(index).ok()?;
        self.selected_shelf()?
            .refs
            .get(index)
            .map(|item| item.path.clone())
    }

    fn select_ref(&mut self, ui: &Panel, index: i32) {
        if let Some(path) = self.ref_path(index) {
            self.selected_ref = Some(path);
            self.push_refs(ui);
        }
    }

    fn remove_ref(&mut self, ui: &Panel) {
        let (Some(shelves), Some(id), Some(path)) = (
            self.shelves.clone(),
            self.selected.clone(),
            self.selected_ref.clone(),
        ) else {
            return;
        };
        match shelves.remove_ref(&id, &path) {
            Ok(_) => {
                ui.set_error(SharedString::new());
                self.selected_ref = None;
                self.reload(ui);
            }
            Err(err) => self.fail(ui, &err.to_string()),
        }
    }

    /// Enter 或双击：用 Windows Shell 打开。已知不存在的路径直接显示错误。
    fn open_ref(&mut self, ui: &Panel) {
        let Some(path) = self.selected_ref.clone() else {
            return;
        };
        if self.exists.get(&path) == Some(&false) {
            self.fail(ui, "路径不存在");
            return;
        }
        ui.set_error(SharedString::new());
        self.spawn_open(ui, path);
    }

    /// Ctrl+Enter：打开所在目录，即路径的上一级。
    fn open_ref_folder(&mut self, ui: &Panel) {
        let Some(path) = self.selected_ref.clone() else {
            return;
        };
        let Some(parent) = Path::new(&path)
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .and_then(Path::to_str)
            .map(str::to_owned)
        else {
            self.fail(ui, "没有所在目录");
            return;
        };
        ui.set_error(SharedString::new());
        self.spawn_open(ui, parent);
    }

    fn spawn_open(&self, ui: &Panel, path: String) {
        let spawned = std::thread::Builder::new()
            .name("lanwork-shelf-open".into())
            .spawn(move || {
                if let Err(err) = launch(&path_target(&path)) {
                    let message = err.message;
                    let _ = slint::invoke_from_event_loop(move || {
                        with_shelves(move |page, ui| page.fail(ui, &message));
                    });
                }
            });
        if let Err(err) = spawned {
            self.fail(ui, &format!("打开失败：{err}"));
        }
    }

    fn open_picker(&mut self, ui: &Panel) {
        match self.todos.lists() {
            Ok(lists) => self.lists = lists,
            Err(err) => {
                self.fail(ui, &err.to_string());
                return;
            }
        }
        ui.set_error(SharedString::new());
        self.choices = todo_choices(&self.lists, "");
        self.push_choices();
        let data = ui.global::<ShelvesPageData>();
        data.set_picker_reset(data.get_picker_reset().wrapping_add(1));
        data.set_picker_open(true);
    }

    fn push_choices(&self) {
        self.choices_model.set_vec(
            self.choices
                .iter()
                .map(|choice| ShelfTodoRow {
                    title: choice.title.as_str().into(),
                    list: choice.list_name.as_str().into(),
                })
                .collect::<Vec<_>>(),
        );
    }

    fn filter_todos(&mut self, keyword: &str) {
        self.choices = todo_choices(&self.lists, keyword);
        self.push_choices();
    }

    fn pick_todo(&mut self, ui: &Panel, index: i32) {
        let data = ui.global::<ShelvesPageData>();
        data.set_picker_open(false);
        let todo_id = usize::try_from(index)
            .ok()
            .and_then(|index| self.choices.get(index))
            .map(|choice| choice.id.clone());
        let (Some(shelves), Some(id), Some(todo_id)) =
            (self.shelves.clone(), self.selected.clone(), todo_id)
        else {
            return;
        };
        match shelves.link_todo(&id, &todo_id) {
            Ok(_) => {
                ui.set_error(SharedString::new());
                self.reload(ui);
            }
            Err(err) => self.fail(ui, &err.to_string()),
        }
    }

    fn unlink_todo(&mut self, ui: &Panel) {
        let (Some(shelves), Some(id)) = (self.shelves.clone(), self.selected.clone()) else {
            return;
        };
        match shelves.unlink_todo(&id) {
            Ok(_) => {
                ui.set_error(SharedString::new());
                self.reload(ui);
            }
            Err(err) => self.fail(ui, &err.to_string()),
        }
    }

    /// 放下的路径加入当前选中的分组；没有分组时由服务先建名为「收纳」的分组。
    fn add_dropped(&mut self, ui: &Panel, incoming: &[IncomingRef], skipped: usize) {
        let Some(shelves) = self.shelves.clone() else {
            self.fail(ui, "收纳服务没有打开");
            return;
        };
        if !incoming.is_empty() {
            match shelves.add_refs(self.selected.as_deref(), incoming) {
                Ok(outcome) => {
                    if let Some(group) = outcome.group {
                        self.selected = Some(group.id);
                    }
                    ui.set_error(SharedString::new());
                    self.reload(ui);
                }
                Err(err) => {
                    self.fail(ui, &err.to_string());
                    return;
                }
            }
        }
        if skipped > 0 {
            self.fail(ui, &format!("{skipped} 个路径不是有效的 Unicode，没有加入"));
        }
    }

    fn ref_press(&mut self, index: i32, x: f32, y: f32) {
        self.press = self.ref_path(index).map(|path| Press { path, x, y });
    }

    /// 移动超过系统拖动阈值就拖出按下的那一条。OLE 的模态循环不能在这次回调里跑，放到下一轮事件循环。
    fn ref_move(&mut self, ui: &Panel, x: f32, y: f32) {
        let Some(press) = self.press.as_ref() else {
            return;
        };
        let scale = ui.window().scale_factor();
        let (cx, cy) = drag_metrics();
        if !passed_drag_threshold((x - press.x) * scale, (y - press.y) * scale, cx, cy) {
            return;
        }
        let Some(press) = self.press.take() else {
            return;
        };
        if self.exists.get(&press.path) == Some(&false) {
            self.fail(ui, "路径不存在");
            return;
        }
        ui.set_error(SharedString::new());
        let origin = (press.x, press.y);
        let path = press.path;
        slint::Timer::single_shot(Duration::ZERO, move || run_drag(path, origin));
    }

    fn drag_finished(
        &mut self,
        ui: &Panel,
        outcome: Result<DragOutcome, String>,
        origin: (f32, f32),
    ) {
        self.dragging = false;
        self.press = None;
        let window = ui.window();
        window.dispatch_event(WindowEvent::PointerReleased {
            position: LogicalPosition::new(origin.0, origin.1),
            button: PointerEventButton::Left,
        });
        window.dispatch_event(WindowEvent::PointerExited);
        match outcome {
            Ok(outcome) if outcome.failed() => {
                self.fail(ui, &format!("拖出失败：{}", outcome.result.message()));
            }
            Ok(_) => {}
            Err(message) => self.fail(ui, &format!("拖出失败：{message}")),
        }
    }
}

/// 在界面线程上进入 OLE 的拖出循环。不能在面板的借用里调用：循环期间窗口消息照常派发。
fn run_drag(path: String, origin: (f32, f32)) {
    with_shelves(|page, _| page.dragging = true);
    let outcome = drag_out(&[PathBuf::from(path)]);
    with_shelves(move |page, ui| page.drag_finished(ui, outcome, origin));
}

/// OLE 放下目标读到路径之后调用。文件夹标记在后台线程里查，结果回到界面线程再写入服务。
pub(crate) fn dropped(paths: Vec<PathBuf>) {
    let spawned = std::thread::Builder::new()
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
            let _ = slint::invoke_from_event_loop(move || {
                with_shelves(move |page, ui| page.add_dropped(ui, &incoming, skipped));
            });
        });
    if let Err(err) = spawned {
        let message = format!("放下的路径没有处理：{err}");
        let _ = slint::invoke_from_event_loop(move || {
            with_shelves(move |page, ui| page.fail(ui, &message));
        });
    }
}

fn count(value: usize) -> i32 {
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

fn wire(ui: &Panel) {
    let data = ui.global::<ShelvesPageData>();
    data.on_select_group(|index| with_shelves(move |page, ui| page.select_group(ui, index)));
    data.on_new_group(|name| {
        let name = name.to_string();
        with_shelves(move |page, ui| page.new_group(ui, &name));
    });
    data.on_rename_group(|name| {
        let name = name.to_string();
        with_shelves(move |page, ui| page.rename_group(ui, &name));
    });
    data.on_move_group(|up| with_shelves(move |page, ui| page.move_group(ui, up)));
    data.on_delete_group(|| with_shelves(ShelvesPage::delete_group));
    data.on_select_ref(|index| with_shelves(move |page, ui| page.select_ref(ui, index)));
    data.on_open_ref(|| with_shelves(ShelvesPage::open_ref));
    data.on_open_ref_folder(|| with_shelves(ShelvesPage::open_ref_folder));
    data.on_remove_ref(|| with_shelves(ShelvesPage::remove_ref));
    data.on_open_picker(|| with_shelves(ShelvesPage::open_picker));
    data.on_close_picker(|| {
        with_shelves(|_, ui| ui.global::<ShelvesPageData>().set_picker_open(false));
    });
    data.on_filter_todos(|text| {
        let text = text.to_string();
        with_shelves(move |page, _| page.filter_todos(&text));
    });
    data.on_pick_todo(|index| with_shelves(move |page, ui| page.pick_todo(ui, index)));
    data.on_unlink_todo(|| with_shelves(ShelvesPage::unlink_todo));
    data.on_ref_press(|index, x, y| with_shelves(move |page, _| page.ref_press(index, x, y)));
    data.on_ref_move(|_, x, y| with_shelves(move |page, ui| page.ref_move(ui, x, y)));
    data.on_ref_release(|| with_shelves(|page, _| page.press = None));
    data.on_visible_changed(|first, visible| {
        with_shelves(move |page, ui| {
            page.first_visible = usize::try_from(first).unwrap_or(0);
            page.visible_count = usize::try_from(visible).unwrap_or(0);
            page.refresh_visible(ui, false);
        });
    });
}
