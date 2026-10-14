//! 面板的收纳页：分组列表、条目、关联待办，以及拖入和拖出。
//!
//! 窗口在 `panel`，版式在 `ui/shelvespage.slint`，OLE 在 `shelf_dnd`，规则在 `lanwork_core::shelves`
//! 和 `lanwork_core::panel`。这里把服务的结果排成界面要的行，并把用户的动作交给服务。
//! 路径引用不打开、不复制、不移动、不删除原文件。网页引用只保存网址和标题，不关闭或移动浏览器标签。
//! 网站图标在保存之后后台获取，失败时图标位留空，不挡住保存和打开。

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use lanwork_core::dispatch::{IconCache, RgbaImage};
use lanwork_core::panel::{
    GroupPick, GroupRow, GroupSelect, ItemMods, ItemSelect, ListViewport, ShelfItemId, ShelfLine,
    TodoChoice, clear_group_checks, clear_items, click_item, context_item, drag_items,
    enter_group_select, first_group_for_todo, group_mods, group_row_height, group_rows,
    group_select_after_delete, hit_row, is_double_click, item_mods, left_window, linked_todo_ids,
    linked_todos, outside_window, pick_group_row, pick_selection, release_moves,
    reordered_group_ids, retain_group_select, retain_items, select_all_groups, select_all_items,
    shelf_lines, todo_choices,
};
use lanwork_core::shelves::{IncomingRef, IncomingWeb, Shelf, ShelfCommands};
use lanwork_core::storage::Store;
use lanwork_core::todos::{TodoCommands, TodoList};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::panel::with_shelves;
use crate::shelf_dnd::{DragOutcome, IncomingDrop};
use crate::shelf_refs::{
    InlineDrag, Moved, ProbeResult, RefsView, drag_error, drag_now, joined_titles, parent_folder,
    prepare_drop, push_links, release_grab, schedule_drag, skipped_message, spawn_launch,
    spawn_open_url, spawn_probe, to_i32,
};
use crate::web_icons::{self, WebIcons};
use crate::{Panel, ShelfGroupRow, ShelfMenuRow, ShelfTodoRow, ShelvesPageData};

#[derive(Clone)]
enum MenuAction {
    Open,
    OpenFolder,
    Remove,
    Move(String),
}

struct InternalDrag {
    origin: (f32, f32),
    paths: Vec<String>,
    urls: Vec<String>,
    ole: bool,
}

/// 放下那一刻的目标分组。OLE 回调在界面线程上，但可能正落在面板的借用里，所以不经过 `with_shelves`
/// 去读选中项，而是由页面在每次重排时把选中的分组 id 放到这里。
static DROP_TARGET: Mutex<Option<String>> = Mutex::new(None);

fn publish_drop_target(selected: Option<&str>) {
    let mut slot = DROP_TARGET.lock().unwrap_or_else(|e| e.into_inner());
    *slot = selected.map(str::to_owned);
}

fn drop_target() -> Option<String> {
    DROP_TARGET
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

pub(crate) struct ShelvesPage {
    shelves: Option<ShelfCommands>,
    todos: TodoCommands,
    icon_cache: Arc<Mutex<IconCache>>,
    web_icons: Arc<WebIcons>,
    store: Store,
    groups_model: Rc<VecModel<ShelfGroupRow>>,
    choices_model: Rc<VecModel<ShelfTodoRow>>,
    view: RefsView,
    list: Vec<Shelf>,
    lists: Vec<TodoList>,
    rows: Vec<GroupRow>,
    selected: Option<String>,
    shown_group: Option<String>,
    choices: Vec<TodoChoice>,
    dragging: bool,
    items: ItemSelect,
    groups_sel: GroupSelect,
    confirm_items: bool,
    confirm_groups: bool,
    menu_model: Rc<VecModel<ShelfMenuRow>>,
    menu_actions: Vec<MenuAction>,
    internal: Option<InternalDrag>,
    drop_index: Option<usize>,
    last_plain: Option<(usize, Instant)>,
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
            web_icons: web_icons::shared(),
            store,
            groups_model: Rc::default(),
            choices_model: Rc::default(),
            view: RefsView::new(),
            list: Vec::new(),
            lists: Vec::new(),
            rows: Vec::new(),
            selected: None,
            shown_group: None,
            choices: Vec::new(),
            dragging: false,
            items: ItemSelect::default(),
            groups_sel: GroupSelect::default(),
            confirm_items: false,
            confirm_groups: false,
            menu_model: Rc::default(),
            menu_actions: Vec::new(),
            internal: None,
            drop_index: None,
            last_plain: None,
        };
        let data = ui.global::<ShelvesPageData>();
        data.set_groups(ModelRc::from(Rc::clone(&page.groups_model)));
        data.set_refs(ModelRc::from(page.view.model()));
        data.set_choices(ModelRc::from(Rc::clone(&page.choices_model)));
        data.set_menu_rows(ModelRc::from(Rc::clone(&page.menu_model)));
        data.set_selected_group(-1);
        data.set_selected_ref(-1);
        data.set_can_open_folder(false);
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
            .map_or(-1, to_i32)
    }

    fn rebuild(&mut self, ui: &Panel) {
        publish_drop_target(self.selected.as_deref());
        if self.list.is_empty() {
            self.groups_sel = GroupSelect::default();
            self.confirm_groups = false;
        }
        let visible_groups: Vec<String> = self.list.iter().map(|shelf| shelf.id.clone()).collect();
        let before_groups = self.groups_sel.ids.clone();
        self.groups_sel = retain_group_select(&self.groups_sel, &visible_groups);
        if self.groups_sel.ids != before_groups {
            self.confirm_groups = false;
        }
        self.rows = group_rows(&self.list, &self.lists);
        self.groups_model.set_vec(
            self.rows
                .iter()
                .map(|row| ShelfGroupRow {
                    name: row.name.as_str().into(),
                    count: to_i32(row.count),
                    todo: joined_titles(&row.todo_titles).into(),
                    checked: self.groups_sel.ids.iter().any(|id| id == &row.id),
                })
                .collect::<Vec<_>>(),
        );
        let data = ui.global::<ShelvesPageData>();
        data.set_selected_group(self.selected_index());

        let changed = self.shown_group != self.selected;
        if changed {
            self.shown_group.clone_from(&self.selected);
            self.items = clear_items();
            self.confirm_items = false;
            self.last_plain = None;
            self.view.clear();
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
        let links = shelf.map_or_else(Vec::new, |shelf| linked_todos(&shelf.todo_ids, &self.lists));
        push_links(&links, &data);
        let (can_up, can_down) = match &self.selected {
            Some(id) => (
                reordered_group_ids(&self.list, id, true).is_some(),
                reordered_group_ids(&self.list, id, false).is_some(),
            ),
            None => (false, false),
        };
        data.set_can_up(can_up);
        data.set_can_down(can_down);

        self.push_refs(ui);
        self.publish_group_chrome(ui);
        self.refresh_visible(ui, true);
    }

    fn lines_of(shelf: Option<&Shelf>) -> Vec<ShelfLine> {
        shelf.map(shelf_lines).unwrap_or_default()
    }

    fn push_refs(&mut self, ui: &Panel) {
        let lines = Self::lines_of(self.selected_shelf());
        let ids = item_ids(&lines);
        let before = self.items.ids.clone();
        self.items = retain_items(&self.items, &ids);
        if self.items.ids != before {
            self.confirm_items = false;
        }
        self.view.apply_items(&lines, &self.items);
        self.view.push(&lines, &ui.global::<ShelvesPageData>());
        self.publish_ref_chrome(ui);
    }

    fn publish_ref_chrome(&self, ui: &Panel) {
        let data = ui.global::<ShelvesPageData>();
        data.set_ref_count(to_i32(self.items.ids.len()));
        data.set_confirm_refs(self.confirm_items);
        data.set_can_open_folder(self.items.ids.iter().any(|id| !id.web));
    }

    fn publish_group_chrome(&self, ui: &Panel) {
        let data = ui.global::<ShelvesPageData>();
        data.set_selecting(self.groups_sel.active);
        data.set_group_selected_count(to_i32(self.groups_sel.ids.len()));
        data.set_confirm_groups(self.confirm_groups);
    }

    fn group_ids(&self) -> Vec<String> {
        self.list.iter().map(|shelf| shelf.id.clone()).collect()
    }

    /// 只给可见的行查路径是否存在和取图标。查询放在后台线程，结果回到界面线程再合并。
    fn refresh_visible(&mut self, ui: &Panel, force: bool) {
        let (Some(shelves), Some(group_id)) = (self.shelves.clone(), self.selected.clone()) else {
            return;
        };
        let lines = self
            .list
            .iter()
            .find(|shelf| shelf.id == group_id)
            .map(shelf_lines)
            .unwrap_or_default();
        let data = ui.global::<ShelvesPageData>();
        let Some(plan) = self.view.refresh(&lines, force, &data) else {
            return;
        };
        let webs = plan.web_urls().to_vec();
        if plan.needs_files() {
            let spawned = spawn_probe(shelves, Arc::clone(&self.icon_cache), plan, move |result| {
                let _ = slint::invoke_from_event_loop(move || {
                    with_shelves(move |page, ui| page.receive(ui, &group_id, result));
                });
            });
            if let Err(err) = spawned {
                self.store
                    .log_warn(&format!("收纳页没有启动路径检查线程：{err}"));
            }
        }
        self.start_web_icons(&webs);
    }

    fn start_web_icons(&self, urls: &[String]) {
        let pending = self.web_icons.claim(urls);
        if pending.is_empty() {
            return;
        }
        if let Err(err) = web_icons::fetch(Arc::clone(&self.web_icons), pending) {
            self.store
                .log_warn(&format!("网站图标的后台任务没有启动：{err}"));
        }
    }

    fn receive(&mut self, ui: &Panel, group_id: &str, result: ProbeResult) {
        if self.selected.as_deref() != Some(group_id) {
            return;
        }
        self.view.receive(result);
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

    fn select_ref(&mut self, ui: &Panel, index: i32) {
        let lines = Self::lines_of(self.selected_shelf());
        if let Ok(index) = usize::try_from(index) {
            self.items = click_item(&self.items, &item_ids(&lines), index, ItemMods::Plain);
            self.confirm_items = false;
        }
        self.push_refs(ui);
    }

    /// 移除选中的引用。多于一条时先确认。不删除原文件，也不关闭浏览器标签。
    fn remove_ref(&mut self, ui: &Panel) {
        let (Some(shelves), Some(group_id)) = (self.shelves.clone(), self.selected.clone()) else {
            return;
        };
        if self.items.ids.is_empty() {
            return;
        }
        if self.items.ids.len() > 1 && !self.confirm_items {
            self.confirm_items = true;
            self.publish_ref_chrome(ui);
            return;
        }
        let chosen = self.items.ids.clone();
        let anchor = self.items.anchor.clone();
        let mut failed = Vec::new();
        let mut first_error = None;
        for item in &chosen {
            let removed = if item.web {
                shelves.remove_web_ref(&group_id, &item.key)
            } else {
                shelves.remove_ref(&group_id, &item.key)
            };
            if let Err(err) = removed {
                failed.push(item.clone());
                if first_error.is_none() {
                    first_error = Some(err.to_string());
                }
            }
        }
        self.items = ItemSelect {
            ids: failed,
            anchor,
        };
        self.confirm_items = false;
        self.close_menu(ui);
        self.reload(ui);
        if let Some(message) = first_error {
            self.fail(ui, &message);
        } else {
            ui.set_error(SharedString::new());
        }
    }

    fn cancel_remove_refs(&mut self, ui: &Panel) {
        self.confirm_items = false;
        self.publish_ref_chrome(ui);
    }

    /// Enter、双击或「打开」：选中的路径用 Windows Shell 打开，网页引用用默认浏览器打开保存的网址。
    fn open_ref(&mut self, ui: &Panel) {
        let items = self.items.ids.clone();
        if items.is_empty() {
            return;
        }
        self.close_menu(ui);
        let mut first_error = None;
        for item in items {
            if item.web {
                if !self.view.has_icon(&item.key)
                    && let Err(err) = web_icons::retry(Arc::clone(&self.web_icons), &item.key)
                {
                    self.store
                        .log_warn(&format!("网站图标的后台任务没有启动：{err}"));
                }
                self.spawn_open_url(ui, item.key);
                continue;
            }
            if self.view.is_missing(&item.key) {
                if first_error.is_none() {
                    first_error = Some("路径不存在".to_owned());
                }
                continue;
            }
            self.spawn_open(ui, item.key);
        }
        if let Some(message) = first_error {
            self.fail(ui, &message);
        } else {
            ui.set_error(SharedString::new());
        }
    }

    /// Ctrl+Enter：打开每条路径引用的上一级。网页引用没有所在目录，跳过。
    fn open_ref_folder(&mut self, ui: &Panel) {
        let paths: Vec<String> = self
            .items
            .ids
            .iter()
            .filter(|item| !item.web)
            .map(|item| item.key.clone())
            .collect();
        if paths.is_empty() {
            return;
        }
        self.close_menu(ui);
        let mut first_error = None;
        let mut opened = false;
        for path in paths {
            match parent_folder(&path) {
                Some(parent) => {
                    opened = true;
                    self.spawn_open(ui, parent);
                }
                None if first_error.is_none() => first_error = Some("没有所在目录".to_owned()),
                None => {}
            }
        }
        if let Some(message) = first_error {
            self.fail(ui, &message);
        } else if opened {
            ui.set_error(SharedString::new());
        }
    }

    fn spawn_open(&self, ui: &Panel, path: String) {
        let spawned = spawn_launch(path, |message| {
            let _ = slint::invoke_from_event_loop(move || {
                with_shelves(move |page, ui| page.fail(ui, &message));
            });
        });
        if let Err(err) = spawned {
            self.fail(ui, &format!("打开失败：{err}"));
        }
    }

    fn spawn_open_url(&self, ui: &Panel, url: String) {
        let spawned = spawn_open_url(url, |message| {
            let _ = slint::invoke_from_event_loop(move || {
                with_shelves(move |page, ui| page.fail(ui, &message));
            });
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

    /// 解除选中分组对第 `index` 条关联待办的关联，其余不动。
    fn unlink_todo(&mut self, ui: &Panel, index: i32) {
        let todo_id = self.selected_shelf().and_then(|shelf| {
            usize::try_from(index)
                .ok()
                .and_then(|index| shelf.todo_ids.get(index))
                .cloned()
        });
        let (Some(shelves), Some(id), Some(todo_id)) =
            (self.shelves.clone(), self.selected.clone(), todo_id)
        else {
            return;
        };
        match shelves.unlink_todo(&id, &todo_id) {
            Ok(_) => {
                ui.set_error(SharedString::new());
                self.reload(ui);
            }
            Err(err) => self.fail(ui, &err.to_string()),
        }
    }

    /// 放下的路径加入放下那一刻选中的分组 `target`；那时没有分组就传空，由服务先建名为「收纳」的分组。
    /// 后台探测期间用户换了分组，也不改变这次的目标，选中项也不被拉回去。
    fn add_dropped(
        &mut self,
        ui: &Panel,
        target: Option<&str>,
        incoming: &[IncomingRef],
        skipped: usize,
    ) {
        let Some(shelves) = self.shelves.clone() else {
            self.fail(ui, "收纳服务没有打开");
            return;
        };
        if !incoming.is_empty() {
            match shelves.add_refs(target, incoming) {
                Ok(outcome) => {
                    if target.is_none()
                        && let Some(group) = outcome.group
                    {
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
        if let Some(message) = skipped_message(skipped) {
            self.fail(ui, &message);
        }
    }

    fn ref_press(&mut self, index: i32, x: f32, y: f32) {
        let lines = Self::lines_of(self.selected_shelf());
        self.view.press_at(&lines, index, x, y, true);
    }

    /// 在窗口里拖过阈值时，先准备挪到别的分组。指针离开窗口才拖出到资源管理器。
    fn ref_move(&mut self, ui: &Panel, x: f32, y: f32, inline: &InlineDrag) {
        match self.view.moved(ui.window().scale_factor(), x, y) {
            Moved::Nothing => {}
            Moved::Missing => self.fail(ui, "路径不存在"),
            Moved::Arm { index, origin } => self.arm_drag(ui, index, origin),
            Moved::Start { path, origin } => {
                ui.set_error(SharedString::new());
                self.dragging = true;
                if !inline.offer(vec![path.clone()], origin) {
                    schedule_drag(
                        vec![path],
                        || {},
                        move |outcome| {
                            with_shelves(move |page, ui| page.drag_finished(ui, outcome, origin));
                        },
                    );
                }
            }
        }
        if self.internal.is_some() {
            self.track_drag(ui, x, y, inline);
        }
    }

    fn arm_drag(&mut self, ui: &Panel, index: usize, origin: (f32, f32)) {
        let lines = Self::lines_of(self.selected_shelf());
        let ids = item_ids(&lines);
        let payload = drag_items(&self.items, &ids, index);
        let pressed_selected = ids
            .get(index)
            .is_some_and(|id| self.items.ids.iter().any(|item| item == id));
        if !pressed_selected {
            self.items = ItemSelect {
                anchor: payload.first().cloned(),
                ids: payload.clone(),
            };
            self.confirm_items = false;
            self.push_refs(ui);
        }
        let paths = payload
            .iter()
            .filter(|item| !item.web)
            .map(|item| item.key.clone())
            .collect();
        let urls = payload
            .iter()
            .filter(|item| item.web)
            .map(|item| item.key.clone())
            .collect();
        self.internal = Some(InternalDrag {
            origin,
            paths,
            urls,
            ole: false,
        });
        self.dragging = true;
        ui.global::<ShelvesPageData>().set_ref_dragging(true);
        ui.set_error(SharedString::new());
    }

    fn track_drag(&mut self, ui: &Panel, x: f32, y: f32, inline: &InlineDrag) {
        let scale = ui.window().scale_factor().max(0.01);
        let size = ui.window().size();
        let width = size.width as f32 / scale;
        let height = size.height as f32 / scale;
        if outside_window(x, y, width, height) {
            self.begin_ole(ui, inline);
            return;
        }
        let data = ui.global::<ShelvesPageData>();
        let heights: Vec<f32> = self
            .rows
            .iter()
            .map(|row| group_row_height(!row.todo_titles.is_empty()))
            .collect();
        let hit = hit_row(
            ListViewport {
                x: data.get_groups_x(),
                y: data.get_groups_y(),
                width: data.get_groups_w(),
                height: data.get_groups_h(),
                content_y: data.get_groups_content_y(),
            },
            &heights,
            x,
            y,
        );
        let source = self.selected_index();
        self.drop_index = hit.filter(|index| to_i32(*index) != source);
        data.set_drop_group(self.drop_index.map(to_i32).unwrap_or(-1));
    }

    fn begin_ole(&mut self, ui: &Panel, inline: &InlineDrag) {
        let Some((origin, stored, already)) = self
            .internal
            .as_ref()
            .map(|drag| (drag.origin, drag.paths.clone(), drag.ole))
        else {
            return;
        };
        let update = left_window(already, self.drop_index, &stored, |path| {
            self.view.is_missing(path)
        });
        // 离开窗口就不再算「松在某个分组上」，无论这次能不能拖出。
        self.drop_index = update.drop_index;
        ui.global::<ShelvesPageData>()
            .set_drop_group(update.drop_index.map(to_i32).unwrap_or(-1));
        if already || update.export.is_empty() {
            return;
        }
        if let Some(drag) = self.internal.as_mut() {
            drag.ole = update.ole;
        }
        ui.set_error(SharedString::new());
        let paths = update.export;
        if !inline.offer(paths.clone(), origin) {
            schedule_drag(
                paths,
                || {},
                move |outcome| {
                    with_shelves(move |page, ui| page.drag_finished(ui, outcome, origin));
                },
            );
        }
    }

    fn finish_internal_drag(&mut self, ui: &Panel) {
        let drag = self.internal.take();
        self.dragging = false;
        self.view.release();
        let data = ui.global::<ShelvesPageData>();
        data.set_ref_dragging(false);
        data.set_drop_group(-1);
        let Some(drag) = drag else {
            self.drop_index = None;
            return;
        };
        if !release_moves(drag.ole, self.drop_index) {
            self.drop_index = None;
            return;
        }
        let target = self
            .drop_index
            .take()
            .and_then(|index| self.list.get(index).map(|shelf| shelf.id.clone()));
        let Some(target) = target else {
            return;
        };
        if self.selected.as_deref() == Some(target.as_str()) {
            return;
        }
        self.move_payload(ui, &drag.paths, &drag.urls, &target);
    }

    fn drag_finished(
        &mut self,
        ui: &Panel,
        outcome: Result<DragOutcome, String>,
        origin: (f32, f32),
    ) {
        self.dragging = false;
        self.internal = None;
        self.drop_index = None;
        self.view.release();
        let data = ui.global::<ShelvesPageData>();
        data.set_ref_dragging(false);
        data.set_drop_group(-1);
        release_grab(ui.window(), origin);
        if let Some(message) = drag_error(outcome) {
            self.fail(ui, &message);
        }
    }

    /// 放下的网址加入放下那一刻选中的分组。没有分组时由服务创建「收纳」。
    fn add_dropped_web(&mut self, ui: &Panel, target: Option<&str>, url: &str, title: &str) {
        let Some(shelves) = self.shelves.clone() else {
            self.fail(ui, "收纳服务没有打开");
            return;
        };
        match shelves.add_web_refs(
            target,
            &[IncomingWeb {
                url: url.to_owned(),
                title: title.to_owned(),
            }],
        ) {
            Ok(outcome) => {
                if target.is_none()
                    && let Some(group) = outcome.group
                {
                    self.selected = Some(group.id);
                }
                ui.set_error(SharedString::new());
                self.reload(ui);
            }
            Err(err) => self.fail(ui, &err.to_string()),
        }
    }

    fn ref_click(&mut self, ui: &Panel, index: i32, ctrl: bool, shift: bool) {
        if ui.global::<ShelvesPageData>().get_ref_dragging() {
            return;
        }
        let lines = Self::lines_of(self.selected_shelf());
        let ids = item_ids(&lines);
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        let mods = item_mods(ctrl, shift);
        let plain = mods == ItemMods::Plain;
        let now = Instant::now();
        let elapsed = self
            .last_plain
            .map(|(_, at)| now.saturating_duration_since(at))
            .unwrap_or_else(|| std::time::Duration::from_secs(10));
        let previous = self.last_plain.map(|(index, _)| index);
        let open = is_double_click(previous, elapsed, index, plain);
        self.last_plain = plain.then_some((index, now));
        self.items = click_item(&self.items, &ids, index, mods);
        self.confirm_items = false;
        self.close_menu(ui);
        self.push_refs(ui);
        if open {
            self.open_ref(ui);
        }
    }

    fn ref_menu(&mut self, ui: &Panel, index: i32, x: f32, y: f32) {
        let lines = Self::lines_of(self.selected_shelf());
        let ids = item_ids(&lines);
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        self.items = context_item(&self.items, &ids, index);
        self.confirm_items = false;
        self.last_plain = None;
        self.push_refs(ui);
        self.open_menu(ui, x, y);
    }

    fn open_menu(&mut self, ui: &Panel, x: f32, y: f32) {
        if self.items.ids.is_empty() {
            self.close_menu(ui);
            return;
        }
        let mut rows = Vec::new();
        let mut actions = Vec::new();
        let push = |rows: &mut Vec<ShelfMenuRow>,
                    actions: &mut Vec<MenuAction>,
                    label: &str,
                    enabled: bool,
                    danger: bool,
                    action: MenuAction| {
            rows.push(ShelfMenuRow {
                label: label.into(),
                enabled,
                danger,
            });
            actions.push(action);
        };
        push(
            &mut rows,
            &mut actions,
            "打开",
            true,
            false,
            MenuAction::Open,
        );
        let folder = self.items.ids.iter().any(|item| !item.web);
        push(
            &mut rows,
            &mut actions,
            "打开所在目录",
            folder,
            false,
            MenuAction::OpenFolder,
        );
        push(
            &mut rows,
            &mut actions,
            "移除引用",
            true,
            true,
            MenuAction::Remove,
        );
        let others: Vec<(String, String)> = self
            .list
            .iter()
            .filter(|shelf| self.selected.as_deref() != Some(shelf.id.as_str()))
            .map(|shelf| (shelf.id.clone(), shelf.name.clone()))
            .collect();
        if !others.is_empty() {
            push(
                &mut rows,
                &mut actions,
                "移动到分组",
                false,
                false,
                MenuAction::Open,
            );
            for (id, name) in others {
                push(
                    &mut rows,
                    &mut actions,
                    &name,
                    true,
                    false,
                    MenuAction::Move(id),
                );
            }
        }
        self.menu_actions = actions;
        self.menu_model.set_vec(rows);
        let data = ui.global::<ShelvesPageData>();
        data.set_menu_x(x - data.get_page_x());
        data.set_menu_y(y - data.get_page_y());
        data.set_menu_open(true);
    }

    fn close_menu(&mut self, ui: &Panel) {
        self.menu_actions.clear();
        self.menu_model.set_vec(Vec::new());
        ui.global::<ShelvesPageData>().set_menu_open(false);
    }

    fn menu_activate(&mut self, ui: &Panel, index: i32) {
        let action = usize::try_from(index)
            .ok()
            .and_then(|index| self.menu_actions.get(index))
            .cloned();
        self.close_menu(ui);
        match action {
            Some(MenuAction::Open) => self.open_ref(ui),
            Some(MenuAction::OpenFolder) => self.open_ref_folder(ui),
            Some(MenuAction::Remove) => self.remove_ref(ui),
            Some(MenuAction::Move(target)) => {
                let paths: Vec<String> = self
                    .items
                    .ids
                    .iter()
                    .filter(|item| !item.web)
                    .map(|item| item.key.clone())
                    .collect();
                let urls: Vec<String> = self
                    .items
                    .ids
                    .iter()
                    .filter(|item| item.web)
                    .map(|item| item.key.clone())
                    .collect();
                self.move_payload(ui, &paths, &urls, &target);
            }
            None => {}
        }
    }

    fn move_payload(&mut self, ui: &Panel, paths: &[String], urls: &[String], target: &str) {
        let (Some(shelves), Some(from)) = (self.shelves.clone(), self.selected.clone()) else {
            return;
        };
        if from == target || (paths.is_empty() && urls.is_empty()) {
            return;
        }
        match shelves.move_refs(&from, target, paths, urls) {
            Ok(_) => {
                self.items = clear_items();
                self.confirm_items = false;
                self.last_plain = None;
                ui.set_error(SharedString::new());
                self.reload(ui);
            }
            Err(err) => self.fail(ui, &err.to_string()),
        }
    }

    fn select_all_refs(&mut self, ui: &Panel) {
        let lines = Self::lines_of(self.selected_shelf());
        self.items = select_all_items(&self.items, &item_ids(&lines));
        self.confirm_items = false;
        self.close_menu(ui);
        self.push_refs(ui);
    }

    fn clear_refs(&mut self, ui: &Panel) {
        self.items = clear_items();
        self.confirm_items = false;
        self.last_plain = None;
        self.close_menu(ui);
        self.push_refs(ui);
    }

    fn group_clicked(&mut self, ui: &Panel, index: i32, ctrl: bool, shift: bool) {
        let visible = self.group_ids();
        let Ok(index) = usize::try_from(index) else {
            return;
        };
        match pick_group_row(
            &self.groups_sel,
            &visible,
            index,
            group_mods(ctrl, shift),
            self.selected.as_deref(),
        ) {
            GroupPick::Open => {
                let Some(id) = visible.get(index).cloned() else {
                    return;
                };
                self.selected = Some(id);
                self.confirm_groups = false;
                self.close_menu(ui);
                self.rebuild(ui);
            }
            GroupPick::Multi(next) => {
                self.groups_sel = next;
                self.confirm_groups = false;
                self.close_menu(ui);
                self.rebuild(ui);
            }
        }
    }

    fn set_group_selecting(&mut self, ui: &Panel, active: bool) {
        if active == self.groups_sel.active {
            return;
        }
        if active {
            if self.list.is_empty() {
                return;
            }
            self.groups_sel = enter_group_select(self.selected.as_deref(), &self.group_ids());
        } else {
            self.groups_sel = GroupSelect::default();
        }
        self.confirm_groups = false;
        self.close_menu(ui);
        self.rebuild(ui);
    }

    fn select_all_groups_action(&mut self, ui: &Panel) {
        if !self.groups_sel.active {
            return;
        }
        self.groups_sel = select_all_groups(&self.groups_sel, &self.group_ids());
        self.confirm_groups = false;
        self.rebuild(ui);
    }

    fn clear_group_checks_action(&mut self, ui: &Panel) {
        if !self.groups_sel.active {
            return;
        }
        self.groups_sel = clear_group_checks(&self.groups_sel);
        self.confirm_groups = false;
        self.rebuild(ui);
    }

    fn ask_delete_groups(&mut self, ui: &Panel) {
        if !self.groups_sel.active || self.groups_sel.ids.is_empty() {
            return;
        }
        self.confirm_groups = true;
        self.publish_group_chrome(ui);
    }

    fn cancel_delete_groups(&mut self, ui: &Panel) {
        self.confirm_groups = false;
        self.publish_group_chrome(ui);
    }

    fn delete_groups(&mut self, ui: &Panel) {
        let Some(shelves) = self.shelves.clone() else {
            return;
        };
        if self.groups_sel.ids.is_empty() {
            return;
        }
        let ids = self.groups_sel.ids.clone();
        let anchor = self.groups_sel.anchor.clone();
        let mut failed = Vec::new();
        let mut first_error = None;
        for id in &ids {
            if let Err(err) = shelves.delete_group(id) {
                failed.push(id.clone());
                if first_error.is_none() {
                    first_error = Some(err.to_string());
                }
            }
        }
        self.groups_sel = group_select_after_delete(anchor.as_deref(), &failed, &failed);
        self.confirm_groups = false;
        self.reload(ui);
        if let Some(message) = first_error {
            self.fail(ui, &message);
        } else {
            ui.set_error(SharedString::new());
        }
    }

    fn float_groups(&mut self, ui: &Panel) {
        if self.groups_sel.ids.is_empty() {
            return;
        }
        let mut first_error = None;
        for id in &self.groups_sel.ids {
            if let Err(message) = crate::shelf_float::open(id)
                && first_error.is_none()
            {
                first_error = Some(message);
            }
        }
        if let Some(message) = first_error {
            self.fail(ui, &message);
        } else {
            ui.set_error(SharedString::new());
        }
    }

    /// 把选中的分组打开成悬浮窗。
    fn float_group(&mut self, ui: &Panel) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        match crate::shelf_float::open(&id) {
            Ok(()) => ui.set_error(SharedString::new()),
            Err(message) => self.fail(ui, &message),
        }
    }
}

/// 后台取到的网站图标。收纳页和悬浮窗都会收到；当前分组没有这条时只先记着。
pub(crate) fn receive_web_icons(icons: Vec<(String, RgbaImage)>) {
    if icons.is_empty() {
        return;
    }
    with_shelves(move |page, ui| {
        page.view.receive_web(&icons);
        page.push_refs(ui);
    });
}

/// OLE 放下目标读到路径或网址之后调用。文件夹标记在后台线程里查，网址直接回到界面线程写入。
pub(crate) fn dropped(incoming: IncomingDrop) {
    match incoming {
        IncomingDrop::Web { url, title } => {
            let target = drop_target();
            let _ = slint::invoke_from_event_loop(move || {
                with_shelves(move |page, ui| {
                    page.add_dropped_web(ui, target.as_deref(), &url, &title);
                });
            });
        }
        IncomingDrop::Paths(paths) => dropped_paths(paths),
    }
}

fn dropped_paths(paths: Vec<PathBuf>) {
    let target = drop_target();
    let spawned = prepare_drop(paths, move |incoming, skipped| {
        let _ = slint::invoke_from_event_loop(move || {
            with_shelves(move |page, ui| {
                page.add_dropped(ui, target.as_deref(), &incoming, skipped);
            });
        });
    });
    if let Err(err) = spawned {
        let message = format!("放下的路径没有处理：{err}");
        let _ = slint::invoke_from_event_loop(move || {
            with_shelves(move |page, ui| page.fail(ui, &message));
        });
    }
}

fn item_ids(lines: &[ShelfLine]) -> Vec<ShelfItemId> {
    lines
        .iter()
        .map(|line| ShelfItemId {
            key: line.key.clone(),
            web: line.web,
        })
        .collect()
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
    data.on_float_group(|| with_shelves(ShelvesPage::float_group));
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
    data.on_unlink_todo(|index| with_shelves(move |page, ui| page.unlink_todo(ui, index)));
    data.on_ref_press(|index, x, y| with_shelves(move |page, _| page.ref_press(index, x, y)));
    data.on_ref_move(|_, x, y| {
        let inline = InlineDrag::new();
        let offered = Rc::clone(&inline);
        with_shelves(move |page, ui| page.ref_move(ui, x, y, &offered));
        if let Some((paths, origin)) = inline.take() {
            drag_now(
                paths,
                || {},
                |outcome| {
                    with_shelves(move |page, ui| page.drag_finished(ui, outcome, origin));
                },
            );
        }
    });
    data.on_ref_release(|| with_shelves(ShelvesPage::finish_internal_drag));
    data.on_set_group_selecting(|active| {
        with_shelves(move |page, ui| page.set_group_selecting(ui, active));
    });
    data.on_group_clicked(|index, ctrl, shift| {
        with_shelves(move |page, ui| page.group_clicked(ui, index, ctrl, shift));
    });
    data.on_select_all_groups(|| with_shelves(ShelvesPage::select_all_groups_action));
    data.on_clear_group_checks(|| with_shelves(ShelvesPage::clear_group_checks_action));
    data.on_ask_delete_groups(|| with_shelves(ShelvesPage::ask_delete_groups));
    data.on_cancel_delete_groups(|| with_shelves(ShelvesPage::cancel_delete_groups));
    data.on_delete_groups(|| with_shelves(ShelvesPage::delete_groups));
    data.on_float_groups(|| with_shelves(ShelvesPage::float_groups));
    data.on_ref_click(|index, ctrl, shift| {
        with_shelves(move |page, ui| page.ref_click(ui, index, ctrl, shift));
    });
    data.on_ref_menu(|index, x, y| {
        with_shelves(move |page, ui| page.ref_menu(ui, index, x, y));
    });
    data.on_menu_activate(|index| with_shelves(move |page, ui| page.menu_activate(ui, index)));
    data.on_close_menu(|| with_shelves(ShelvesPage::close_menu));
    data.on_select_all_refs(|| with_shelves(ShelvesPage::select_all_refs));
    data.on_clear_refs(|| with_shelves(ShelvesPage::clear_refs));
    data.on_cancel_remove_refs(|| with_shelves(ShelvesPage::cancel_remove_refs));
    data.on_visible_changed(|first, visible| {
        with_shelves(move |page, ui| {
            page.view.set_visible(first, visible);
            page.refresh_visible(ui, false);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_drop_target_is_what_was_selected_when_it_was_published() {
        publish_drop_target(Some("g1"));
        let at_drop = drop_target();
        publish_drop_target(Some("g2"));
        assert_eq!(at_drop.as_deref(), Some("g1"));
        assert_eq!(drop_target().as_deref(), Some("g2"));
        publish_drop_target(None);
        assert_eq!(drop_target(), None);
    }
}
