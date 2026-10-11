//! 面板的收纳页：分组列表、条目、关联待办，以及拖入和拖出。
//!
//! 窗口在 `panel`，版式在 `ui/shelvespage.slint`，OLE 在 `shelf_dnd`，规则在 `lanwork_core::shelves`
//! 和 `lanwork_core::panel`。这里把服务的结果排成界面要的行，并把用户的动作交给服务。
//! 只保存路径引用；不打开、不复制、不移动、不删除被引用的文件。

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use lanwork_core::dispatch::IconCache;
use lanwork_core::panel::{
    GroupRow, TodoChoice, first_group_for_todo, group_rows, linked_todo_ids, pick_selection,
    reordered_group_ids, todo_choices,
};
use lanwork_core::shelves::{IncomingRef, Shelf, ShelfCommands, ShelfRef};
use lanwork_core::storage::Store;
use lanwork_core::todos::{TodoCommands, TodoList};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::panel::with_shelves;
use crate::shelf_dnd::DragOutcome;
use crate::shelf_refs::{
    InlineDrag, Moved, ProbeResult, RefsView, drag_error, drag_now, parent_folder, prepare_drop,
    release_grab, schedule_drag, skipped_message, spawn_launch, spawn_probe, to_i32,
};
use crate::{Panel, ShelfGroupRow, ShelfTodoRow, ShelvesPageData};

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
            choices_model: Rc::default(),
            view: RefsView::new(),
            list: Vec::new(),
            lists: Vec::new(),
            rows: Vec::new(),
            selected: None,
            shown_group: None,
            choices: Vec::new(),
            dragging: false,
        };
        let data = ui.global::<ShelvesPageData>();
        data.set_groups(ModelRc::from(Rc::clone(&page.groups_model)));
        data.set_refs(ModelRc::from(page.view.model()));
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
            .map_or(-1, to_i32)
    }

    fn rebuild(&mut self, ui: &Panel) {
        publish_drop_target(self.selected.as_deref());
        self.rows = group_rows(&self.list, &self.lists);
        self.groups_model.set_vec(
            self.rows
                .iter()
                .map(|row| ShelfGroupRow {
                    name: row.name.as_str().into(),
                    count: to_i32(row.count),
                    todo: row.todo_title.as_str().into(),
                })
                .collect::<Vec<_>>(),
        );
        let data = ui.global::<ShelvesPageData>();
        data.set_selected_group(self.selected_index());

        let changed = self.shown_group != self.selected;
        if changed {
            self.shown_group.clone_from(&self.selected);
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

        self.push_refs(ui);
        self.refresh_visible(ui, true);
    }

    fn refs(&self) -> &[ShelfRef] {
        self.selected_shelf()
            .map_or(&[], |shelf| shelf.refs.as_slice())
    }

    fn push_refs(&self, ui: &Panel) {
        self.view.push(self.refs(), &ui.global::<ShelvesPageData>());
    }

    /// 只给可见的行查路径是否存在和取图标。查询放在后台线程，结果回到界面线程再合并。
    fn refresh_visible(&mut self, ui: &Panel, force: bool) {
        let (Some(shelves), Some(group_id)) = (self.shelves.clone(), self.selected.clone()) else {
            return;
        };
        let refs = self
            .list
            .iter()
            .find(|shelf| shelf.id == group_id)
            .map_or(&[][..], |shelf| shelf.refs.as_slice());
        let data = ui.global::<ShelvesPageData>();
        let Some(plan) = self.view.refresh(refs, force, &data) else {
            return;
        };
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
        if let Some(shelf) = self.selected_shelf() {
            let refs = shelf.refs.clone();
            self.view.select(&refs, index);
            self.push_refs(ui);
        }
    }

    fn remove_ref(&mut self, ui: &Panel) {
        let (Some(shelves), Some(id), Some(path)) = (
            self.shelves.clone(),
            self.selected.clone(),
            self.view.selected().map(str::to_owned),
        ) else {
            return;
        };
        match shelves.remove_ref(&id, &path) {
            Ok(_) => {
                ui.set_error(SharedString::new());
                self.view.deselect();
                self.reload(ui);
            }
            Err(err) => self.fail(ui, &err.to_string()),
        }
    }

    /// Enter 或双击：用 Windows Shell 打开。已知不存在的路径直接显示错误。
    fn open_ref(&mut self, ui: &Panel) {
        let Some(path) = self.view.selected().map(str::to_owned) else {
            return;
        };
        if self.view.is_missing(&path) {
            self.fail(ui, "路径不存在");
            return;
        }
        ui.set_error(SharedString::new());
        self.spawn_open(ui, path);
    }

    /// Ctrl+Enter：打开所在目录，即路径的上一级。
    fn open_ref_folder(&mut self, ui: &Panel) {
        let Some(path) = self.view.selected() else {
            return;
        };
        let Some(parent) = parent_folder(path) else {
            self.fail(ui, "没有所在目录");
            return;
        };
        ui.set_error(SharedString::new());
        self.spawn_open(ui, parent);
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
        let refs = self.selected_shelf().map(|shelf| shelf.refs.clone());
        if let Some(refs) = refs {
            self.view.press(&refs, index, x, y);
        }
    }

    /// 移动超过系统拖动阈值就拖出按下的那一条。同步执行时把路径交给 `inline`，由回调在借用结束后
    /// 开始拖出；动作被推迟时退回下一轮事件循环。
    fn ref_move(&mut self, ui: &Panel, x: f32, y: f32, inline: &InlineDrag) {
        match self.view.moved(ui.window().scale_factor(), x, y) {
            Moved::Nothing => {}
            Moved::Missing => self.fail(ui, "路径不存在"),
            Moved::Start { path, origin } => {
                ui.set_error(SharedString::new());
                self.dragging = true;
                if !inline.offer(path.clone(), origin) {
                    schedule_drag(
                        path,
                        || {},
                        move |outcome| {
                            with_shelves(move |page, ui| page.drag_finished(ui, outcome, origin));
                        },
                    );
                }
            }
        }
    }

    fn drag_finished(
        &mut self,
        ui: &Panel,
        outcome: Result<DragOutcome, String>,
        origin: (f32, f32),
    ) {
        self.dragging = false;
        self.view.release();
        release_grab(ui.window(), origin);
        if let Some(message) = drag_error(outcome) {
            self.fail(ui, &message);
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

/// OLE 放下目标读到路径之后调用。文件夹标记在后台线程里查，结果回到界面线程再写入服务。
pub(crate) fn dropped(paths: Vec<PathBuf>) {
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
    data.on_unlink_todo(|| with_shelves(ShelvesPage::unlink_todo));
    data.on_ref_press(|index, x, y| with_shelves(move |page, _| page.ref_press(index, x, y)));
    data.on_ref_move(|_, x, y| {
        let inline = InlineDrag::new();
        let offered = Rc::clone(&inline);
        with_shelves(move |page, ui| page.ref_move(ui, x, y, &offered));
        if let Some((path, origin)) = inline.take() {
            drag_now(
                path,
                || {},
                |outcome| {
                    with_shelves(move |page, ui| page.drag_finished(ui, outcome, origin));
                },
            );
        }
    });
    data.on_ref_release(|| with_shelves(|page, _| page.view.release()));
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
