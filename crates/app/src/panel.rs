//! 面板窗口：标签框架、待办页和定位接口。
//!
//! 窗口启动时创建并保持隐藏。待办页调用 `TodoCommands`，规则在服务里，这里只把命令结果
//! 排成界面要的行。便签页在 `notes_page`。面板搜索框、热角、收纳页、GitHub 页和设置页还没有。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use lanwork_core::CivilDate;
use lanwork_core::apps::{LaunchTarget, launch};
use lanwork_core::notes::NoteCommands;
use lanwork_core::panel::{
    self, PanelTab, PanelTarget, RECURRENCE_CHOICES, Row, TodoView, format_date, panel_origin,
    panel_size, parse_due, parse_remind, recurrence_from_choice, recurrence_index, reordered_ids,
};
use lanwork_core::shell::{Backdrop, WorkArea, solid_rgb, to_physical};
use lanwork_core::storage::Store;
use lanwork_core::todos::{NewTodo, TodoCommands, TodoItem, TodoList};
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::bar_win;
use crate::host::{local_today, unix_now_ms};
use crate::notes_page::NotesPage;
use crate::{Panel, PanelDetail, PanelList, PanelRow};

/// 显示后这段时间内的失焦不收起。托盘菜单关闭时焦点会迟到地变化一次。
const BLUR_GRACE: Duration = Duration::from_millis(400);

thread_local! {
    static PANEL: RefCell<Option<Controller>> = const { RefCell::new(None) };
}

/// 在界面线程上取面板。回调里可能已经借用着，这时放到下一轮事件循环。
pub(crate) fn with_panel(action: impl FnOnce(&mut Controller) + 'static) {
    let deferred = PANEL.with(|cell| match cell.try_borrow_mut() {
        Ok(mut slot) => {
            if let Some(panel) = slot.as_mut() {
                action(panel);
            }
            None
        }
        Err(_) => Some(action),
    });
    if let Some(action) = deferred {
        slint::Timer::single_shot(Duration::ZERO, move || with_panel(action));
    }
}

/// 便签页的动作。和面板共用同一个借用，所以也走 [`with_panel`]。
pub(crate) fn with_notes(action: impl FnOnce(&mut NotesPage, &Panel) + 'static) {
    with_panel(move |panel| action(&mut panel.notes, &panel.ui));
}

/// 启动时调用一次。窗口保持隐藏，直到托盘或搜索结果打开它。
pub(crate) fn install(ui: Panel, todos: TodoCommands, notes: Option<NoteCommands>, store: Store) {
    let tab_titles: Vec<slint::SharedString> =
        PanelTab::ALL.iter().map(|tab| tab.title().into()).collect();
    ui.set_tab_titles(ModelRc::from(Rc::new(VecModel::from(tab_titles))));
    let lists_model = Rc::new(VecModel::<PanelList>::default());
    let rows_model = Rc::new(VecModel::<PanelRow>::default());
    ui.set_lists(ModelRc::from(Rc::clone(&lists_model)));
    ui.set_rows(ModelRc::from(Rc::clone(&rows_model)));
    wire(&ui);

    ui.window().on_winit_window_event(|_, event| {
        match event {
            winit::event::WindowEvent::Focused(true) => with_panel(|panel| panel.focused = true),
            winit::event::WindowEvent::Focused(false) => with_panel(Controller::blur),
            _ => {}
        }
        EventResult::Propagate
    });

    let ready = ui.clone_strong();
    let spawned = slint::spawn_local(async move {
        let window = ready.window().winit_window().await;
        if let Ok(window) = window {
            use winit::platform::windows::WindowExtWindows as _;
            window.set_skip_taskbar(true);
        }
        with_panel(Controller::window_ready);
    });
    if let Err(err) = spawned {
        store.log_warn(&format!("面板窗口初始化未排队：{err}"));
    }

    let controller = Controller {
        notes: NotesPage::new(&ui, notes, store.clone()),
        ui,
        todos,
        store,
        lists_model,
        rows_model,
        tab: PanelTab::Todo,
        lists: Vec::new(),
        view: None,
        rows: Vec::new(),
        selected: None,
        detail_version: 0,
        error: None,
        visible: false,
        shown_at: None,
        focused: false,
        dark: false,
        hwnd_ready: false,
    };
    PANEL.with(|cell| *cell.borrow_mut() = Some(controller));
}

/// 托盘「打开面板」「设置」和面板热键。`tab` 为空时保持收起前的标签。
pub(crate) fn show(tab: Option<PanelTab>) {
    with_panel(move |panel| panel.show(tab));
}

/// 打开面板并定位到目标。搜索结果和到期通知都走这里。
pub(crate) fn open_target(target: PanelTarget) {
    with_panel(move |panel| panel.open_target(&target));
}

/// 面板外的动作失败时，把原因显示在面板上。
pub(crate) fn show_error(message: &str) {
    let message = message.to_owned();
    with_panel(move |panel| {
        panel.show(None);
        panel.error = Some(message);
        panel.push_error();
    });
}

/// 托盘「新建便签」：创建一篇，打开面板的便签页并选中它。
pub(crate) fn new_note() {
    with_panel(Controller::new_note);
}

/// 便签有变更消息时调用。面板隐藏时不重画，下次显示再读。
pub(crate) fn on_notes_changed() {
    with_panel(|panel| {
        if panel.visible && panel.tab == PanelTab::Notes {
            panel.notes.reload(&panel.ui);
            panel.notes.refresh_editor(&panel.ui);
        }
    });
}

/// 退出前保存便签页里没写盘的修改。返回保存之后仍没写盘的窗口数，有的话面板显示出来，让用户选择。
pub(crate) fn flush_notes_for_quit() -> usize {
    PANEL.with(|cell| {
        let Ok(mut slot) = cell.try_borrow_mut() else {
            return 1;
        };
        let Some(panel) = slot.as_mut() else {
            return 0;
        };
        let blocked = panel.notes.flush_for_quit(&panel.ui);
        if blocked > 0 {
            panel.show(Some(PanelTab::Notes));
        }
        blocked
    })
}

pub(crate) fn apply_theme(dark: bool) {
    with_panel(move |panel| {
        panel.dark = dark;
        panel.ui.set_dark(dark);
        panel.refresh_backdrop();
    });
}

/// 待办有变更消息时调用。面板隐藏时不重画，下次显示再读。
pub(crate) fn on_todos_changed() {
    with_panel(|panel| {
        if panel.visible {
            panel.reload();
        }
    });
}

fn wire(ui: &Panel) {
    ui.on_tab_selected(|index| {
        if let Some(tab) = PanelTab::from_index(index) {
            with_panel(move |panel| panel.select_tab(tab));
        }
    });
    ui.on_close_panel(|| with_panel(Controller::hide));
    ui.on_escape(|| with_panel(Controller::hide));
    ui.on_drag_window(|| with_panel(Controller::drag));
    ui.on_select_list(|index| with_panel(move |panel| panel.select_list(index)));
    ui.on_select_trash(|| with_panel(|panel| panel.select_view(TodoView::Trash)));
    ui.on_create_list(|name| {
        let name = name.to_string();
        with_panel(move |panel| panel.create_list(&name));
    });
    ui.on_delete_list(|| with_panel(Controller::delete_list));
    ui.on_add_item(|title| {
        let title = title.to_string();
        with_panel(move |panel| panel.add_item(&title));
    });
    ui.on_select_row(|index| with_panel(move |panel| panel.select_row(index)));
    ui.on_toggle_complete(|index| with_panel(move |panel| panel.complete_row(index)));
    ui.on_toggle_current(|| with_panel(Controller::toggle_current));
    ui.on_open_source(|| with_panel(Controller::open_source));
    ui.on_move_item(|up| with_panel(move |panel| panel.move_selected(up)));
    ui.on_delete_item(|| with_panel(Controller::delete_selected));
    ui.on_restore_item(|| with_panel(Controller::restore_selected));
    ui.on_purge_item(|| with_panel(Controller::purge_selected));
    ui.on_commit_title(|text| {
        let text = text.to_string();
        with_panel(move |panel| panel.commit_title(&text));
    });
    ui.on_commit_due(|text| {
        let text = text.to_string();
        with_panel(move |panel| panel.commit_due(&text));
    });
    ui.on_commit_remind(|text| {
        let text = text.to_string();
        with_panel(move |panel| panel.commit_remind(&text));
    });
    ui.on_choose_recurrence(|index| with_panel(move |panel| panel.choose_recurrence(index)));
    ui.on_commit_until(|text| {
        let text = text.to_string();
        with_panel(move |panel| panel.commit_until(&text));
    });
}

pub(crate) struct Controller {
    ui: Panel,
    notes: NotesPage,
    todos: TodoCommands,
    store: Store,
    lists_model: Rc<VecModel<PanelList>>,
    rows_model: Rc<VecModel<PanelRow>>,
    tab: PanelTab,
    lists: Vec<TodoList>,
    view: Option<TodoView>,
    rows: Vec<Row>,
    selected: Option<String>,
    detail_version: i32,
    error: Option<String>,
    visible: bool,
    shown_at: Option<Instant>,
    /// 这次显示之后窗口拿到过焦点。显示瞬间的 `Focused(false)` 不收起。
    focused: bool,
    dark: bool,
    hwnd_ready: bool,
}

impl Controller {
    fn window_ready(&mut self) {
        self.hwnd_ready = true;
        if let Some(hwnd) = self.hwnd()
            && !bar_win::remove_caption_buttons(hwnd)
        {
            self.store
                .log_warn("面板样式子类没有装上，显示时可能出现标题栏按钮");
        }
        self.refresh_backdrop();
    }

    fn hwnd(&self) -> Option<windows::Win32::Foundation::HWND> {
        if self.hwnd_ready {
            bar_win::hwnd_of(self.ui.window())
        } else {
            None
        }
    }

    fn refresh_backdrop(&mut self) {
        let actual = match self.hwnd() {
            Some(hwnd) => bar_win::apply_dwm(hwnd, bar_win::current_backdrop(), self.dark),
            None => Backdrop::Solid,
        };
        self.ui.set_acrylic(actual == Backdrop::Acrylic);
        self.ui.set_solid_color(rgb(solid_rgb(self.dark)));
    }

    fn show(&mut self, tab: Option<PanelTab>) {
        if let Some(tab) = tab {
            self.tab = tab;
        }
        self.prepare();
        self.present();
    }

    fn open_target(&mut self, target: &PanelTarget) {
        self.tab = match target {
            PanelTarget::Todo { .. } => PanelTab::Todo,
            PanelTarget::Note { .. } => PanelTab::Notes,
        };
        self.prepare();
        match target {
            PanelTarget::Todo { item_id } => self.locate(item_id),
            PanelTarget::Note { note_id } => self.notes.locate(&self.ui, note_id),
        }
        self.present();
    }

    fn new_note(&mut self) {
        self.tab = PanelTab::Notes;
        self.prepare();
        self.notes.create_new(&self.ui);
        self.present();
    }

    /// 读最新数据并推给界面。窗口还没显示时也调用，这样第一帧就是对的。
    fn prepare(&mut self) {
        self.error = None;
        if let Err(err) = self.todos.ensure_inbox() {
            self.error = Some(err.to_string());
        }
        self.reload();
        self.ui
            .set_tab(i32::try_from(self.tab.index()).unwrap_or(0));
        self.ui.set_empty_label(self.tab.empty_label().into());
        self.push_error();
        if self.tab == PanelTab::Notes {
            self.notes.entered(&self.ui);
        }
    }

    fn present(&mut self) {
        let (work, dpi) = bar_win::cursor_monitor().unwrap_or((
            WorkArea {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            lanwork_core::shell::BASE_DPI,
        ));
        let (width, height) = panel_size(work, dpi);
        self.ui.set_panel_width(px(width));
        self.ui.set_panel_height(px(height));
        let origin = panel_origin(work, to_physical(width, dpi), to_physical(height, dpi));
        if !self.visible {
            self.focused = false;
            self.ui
                .window()
                .set_position(slint::PhysicalPosition::new(origin.0, origin.1));
        }
        self.refresh_backdrop();
        self.visible = true;
        self.shown_at = Some(Instant::now());
        if let Err(err) = self.ui.show() {
            self.visible = false;
            self.store.log_warn(&format!("面板没有显示：{err}"));
            return;
        }
        if let Some(hwnd) = self.hwnd() {
            if bar_win::window_origin(hwnd) != Some(origin) && !self.focused {
                bar_win::move_window(hwnd, origin.0, origin.1);
            }
            bar_win::bring_to_front(hwnd);
        }
        self.ui.invoke_focus_root();
        self.ui.invoke_ensure_row_visible(self.selected_index());
    }

    fn hide(&mut self) {
        self.visible = false;
        self.error = None;
        let _ = self.ui.hide();
    }

    fn blur(&mut self) {
        let settling = self
            .shown_at
            .is_some_and(|shown| shown.elapsed() < BLUR_GRACE);
        if self.visible && self.focused && !settling {
            self.hide();
        }
    }

    fn drag(&mut self) {
        let _ = self.ui.window().with_winit_window(|window| {
            let _ = window.drag_window();
        });
    }

    fn select_tab(&mut self, tab: PanelTab) {
        self.tab = tab;
        self.ui.set_tab(i32::try_from(tab.index()).unwrap_or(0));
        self.ui.set_empty_label(tab.empty_label().into());
        if tab == PanelTab::Notes {
            self.notes.entered(&self.ui);
        } else {
            self.push_error();
        }
        self.ui.invoke_focus_root();
    }

    fn today(&mut self) -> Option<CivilDate> {
        let today = local_today();
        if today.is_none() {
            self.store.log_warn("读不到本地日期，面板不标逾期");
        }
        today
    }

    /// 从服务重新读清单，保持当前视图和选中的条目，然后推给界面。
    fn reload(&mut self) {
        match self.todos.lists() {
            Ok(lists) => self.lists = lists,
            Err(err) => {
                self.error = Some(err.to_string());
                self.push_error();
                return;
            }
        }
        let keep = self
            .view
            .clone()
            .filter(|view| panel::view_exists(&self.lists, view));
        self.view = keep.or_else(|| panel::default_view(&self.lists));
        self.rebuild();
    }

    /// 由 `lists` 和 `view` 重排界面数据，不读服务。
    fn rebuild(&mut self) {
        let today = self.today();
        let now = unix_now_ms();
        let entries = panel::list_entries(&self.lists);
        let selected_list = match &self.view {
            Some(TodoView::List(id)) => entries.iter().position(|entry| &entry.id == id),
            _ => None,
        };
        let can_delete = selected_list.is_some_and(|index| !entries[index].inbox);
        let models: Vec<PanelList> = entries
            .iter()
            .map(|entry| PanelList {
                name: entry.name.as_str().into(),
                count: i32::try_from(entry.open).unwrap_or(i32::MAX),
            })
            .collect();
        self.lists_model.set_vec(models);
        self.ui.set_selected_list(
            selected_list.map_or(-1, |index| i32::try_from(index).unwrap_or(-1)),
        );
        self.ui
            .set_trash_selected(matches!(self.view, Some(TodoView::Trash)));
        self.ui
            .set_trash_count(i32::try_from(panel::trash_count(&self.lists)).unwrap_or(i32::MAX));
        self.ui.set_can_delete_list(can_delete);

        self.rows = match &self.view {
            Some(view) => panel::rows(&self.lists, view, today, now),
            None => Vec::new(),
        };
        if self
            .selected
            .as_ref()
            .is_some_and(|id| !self.rows.iter().any(|row| &row.id == id))
        {
            self.selected = None;
        }
        let models: Vec<PanelRow> = self.rows.iter().map(row_model).collect();
        self.rows_model.set_vec(models);
        self.ui.set_selected_row(self.selected_index());
        self.push_detail(false);
    }

    fn selected_index(&self) -> i32 {
        self.selected
            .as_ref()
            .and_then(|id| self.rows.iter().position(|row| &row.id == id))
            .map_or(-1, |index| i32::try_from(index).unwrap_or(-1))
    }

    fn selected_item(&self) -> Option<(&TodoList, &TodoItem)> {
        let id = self.selected.as_ref()?;
        self.lists.iter().find_map(|list| {
            list.items
                .iter()
                .find(|item| &item.id == id)
                .map(|item| (list, item))
        })
    }

    /// `reset` 为真时输入框回到条目里的值；写入失败时不重置，保留用户正在改的文字。
    fn push_detail(&mut self, reset: bool) {
        if reset {
            self.detail_version = self.detail_version.wrapping_add(1);
        }
        self.ui.set_detail_version(self.detail_version);
        let Some((list, item)) = self.selected_item() else {
            self.ui.set_detail(PanelDetail::default());
            return;
        };
        let detail = PanelDetail {
            title: item.title.as_str().into(),
            due: item.due.map(format_date).unwrap_or_default().into(),
            remind: item
                .remind_at
                .map(|time| format!("{:02}:{:02}", time.hour(), time.minute()))
                .unwrap_or_default()
                .into(),
            recurrence: i32::try_from(recurrence_index(item.recurrence.as_ref())).unwrap_or(0),
            until: item
                .recurrence
                .as_ref()
                .and_then(|recurrence| recurrence.until)
                .map(format_date)
                .unwrap_or_default()
                .into(),
            has_source: item.source.is_some(),
            current: item.current,
            can_up: reordered_ids(&self.lists, &list.id, &item.id, true).is_some(),
            can_down: reordered_ids(&self.lists, &list.id, &item.id, false).is_some(),
        };
        self.ui.set_detail(detail);
    }

    fn push_error(&mut self) {
        self.ui
            .set_error(self.error.clone().unwrap_or_default().into());
    }

    /// 命令成功后清掉错误并重读；失败时停在当前状态并显示错误。
    fn finish(&mut self, result: Result<(), String>, reset: bool) {
        match result {
            Ok(()) => {
                self.error = None;
                self.reload();
                self.push_detail(reset);
            }
            Err(message) => {
                self.error = Some(message);
                self.reload();
            }
        }
        self.push_error();
    }

    fn locate(&mut self, item_id: &str) {
        let today = self.today();
        match panel::locate(&self.lists, item_id, today, unix_now_ms()) {
            Some((view, _)) => {
                self.view = Some(view);
                self.selected = Some(item_id.to_owned());
                self.rebuild();
                self.push_detail(true);
            }
            None => {
                self.error = Some("找不到待办".to_owned());
                self.push_error();
            }
        }
    }

    fn select_list(&mut self, index: i32) {
        let Some(list) = usize::try_from(index)
            .ok()
            .and_then(|index| self.lists.get(index))
        else {
            return;
        };
        self.select_view(TodoView::List(list.id.clone()));
    }

    fn select_view(&mut self, view: TodoView) {
        if self.view.as_ref() == Some(&view) {
            return;
        }
        self.view = Some(view);
        self.selected = None;
        self.error = None;
        self.rebuild();
        self.push_detail(true);
        self.push_error();
        self.ui.invoke_scroll_to_top();
    }

    fn select_row(&mut self, index: i32) {
        let Some(row) = usize::try_from(index)
            .ok()
            .and_then(|index| self.rows.get(index))
        else {
            return;
        };
        self.selected = Some(row.id.clone());
        self.ui.set_selected_row(index);
        self.push_detail(true);
    }

    fn row_id(&self, index: i32) -> Option<String> {
        usize::try_from(index)
            .ok()
            .and_then(|index| self.rows.get(index))
            .map(|row| row.id.clone())
    }

    fn create_list(&mut self, name: &str) {
        match self.todos.create_list(name) {
            Ok(id) => {
                self.view = Some(TodoView::List(id));
                self.selected = None;
                self.finish(Ok(()), true);
                self.ui.invoke_clear_new_list();
            }
            Err(err) => self.finish(Err(err.to_string()), false),
        }
    }

    fn delete_list(&mut self) {
        let Some(TodoView::List(id)) = self.view.clone() else {
            return;
        };
        let result = self.todos.delete_list(&id).map_err(|err| err.to_string());
        if result.is_ok() {
            self.view = None;
            self.selected = None;
        }
        self.finish(result, true);
    }

    fn add_item(&mut self, title: &str) {
        let Some(TodoView::List(list_id)) = self.view.clone() else {
            return;
        };
        let draft = NewTodo {
            title: title.to_owned(),
            due: None,
            remind_at: None,
            recurrence: None,
            source: None,
        };
        match self.todos.create_item(&list_id, draft) {
            Ok(id) => {
                self.selected = Some(id);
                self.finish(Ok(()), true);
                self.ui.invoke_clear_new_item();
                self.ui.invoke_ensure_row_visible(self.selected_index());
            }
            Err(err) => self.finish(Err(err.to_string()), false),
        }
    }

    fn complete_row(&mut self, index: i32) {
        let Some(id) = self.row_id(index) else {
            return;
        };
        let completed = self
            .rows
            .get(usize::try_from(index).unwrap_or(0))
            .is_some_and(|row| row.completed);
        let result = if completed {
            self.todos.uncomplete_item(&id)
        } else {
            self.todos.complete_item(&id)
        }
        .map_err(|err| err.to_string());
        self.finish(result, true);
    }

    fn toggle_current(&mut self) {
        let Some((_, item)) = self.selected_item() else {
            return;
        };
        let (id, current) = (item.id.clone(), item.current);
        let result = if current {
            self.todos.clear_current()
        } else {
            self.todos.set_current(&id)
        }
        .map_err(|err| err.to_string());
        self.finish(result, true);
    }

    fn open_source(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let url = match self.todos.open_source(&id) {
            Ok(url) => url,
            Err(err) => {
                self.finish(Err(err.to_string()), false);
                return;
            }
        };
        self.error = None;
        self.push_error();
        let spawned = std::thread::Builder::new()
            .name("lanwork-open-source".into())
            .spawn(move || {
                let result = launch(&LaunchTarget::Url { url });
                if let Err(err) = result {
                    let _ = slint::invoke_from_event_loop(move || {
                        with_panel(move |panel| {
                            panel
                                .store
                                .log_warn(&format!("打开来源失败：{}", err.message));
                            panel.error = Some(err.message);
                            panel.push_error();
                        });
                    });
                }
            });
        if let Err(err) = spawned {
            self.error = Some(format!("打开失败：{err}"));
            self.push_error();
        }
    }

    fn move_selected(&mut self, up: bool) {
        let Some((list, item)) = self.selected_item() else {
            return;
        };
        let Some(ids) = reordered_ids(&self.lists, &list.id, &item.id, up) else {
            return;
        };
        let list_id = list.id.clone();
        let result = self
            .todos
            .reorder_items(&list_id, &ids)
            .map_err(|err| err.to_string());
        self.finish(result, false);
        self.ui.invoke_ensure_row_visible(self.selected_index());
    }

    fn delete_selected(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let result = self.todos.soft_delete(&id).map_err(|err| err.to_string());
        self.finish(result, true);
    }

    fn restore_selected(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let result = self.todos.restore(&id).map_err(|err| err.to_string());
        self.finish(result, true);
    }

    fn purge_selected(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let result = self.todos.purge(&id).map_err(|err| err.to_string());
        self.finish(result, true);
    }

    fn commit_title(&mut self, text: &str) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let result = self
            .todos
            .rename_item(&id, text)
            .map_err(|err| err.to_string());
        self.finish(result, true);
    }

    fn commit_due(&mut self, text: &str) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let result = parse_due(text)
            .map_err(|err| err.to_string())
            .and_then(|due| self.todos.set_due(&id, due).map_err(|err| err.to_string()));
        self.finish(result, true);
    }

    fn commit_remind(&mut self, text: &str) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let result = parse_remind(text)
            .map_err(|err| err.to_string())
            .and_then(|time| {
                match time {
                    Some((hour, minute)) => self.todos.set_reminder(&id, hour, minute),
                    None => self.todos.clear_reminder(&id),
                }
                .map_err(|err| err.to_string())
            });
        self.finish(result, true);
    }

    fn choose_recurrence(&mut self, index: i32) {
        let Some((_, item)) = self.selected_item() else {
            return;
        };
        let id = item.id.clone();
        let existing = item.recurrence.clone();
        let choice = usize::try_from(index)
            .ok()
            .filter(|choice| *choice < RECURRENCE_CHOICES.len())
            .unwrap_or(0);
        let recurrence = recurrence_from_choice(
            choice,
            existing.as_ref().and_then(|recurrence| recurrence.until),
            existing.as_ref(),
        );
        let result = self
            .todos
            .set_recurrence(&id, recurrence)
            .map_err(|err| err.to_string());
        self.finish(result, true);
    }

    fn commit_until(&mut self, text: &str) {
        let Some((_, item)) = self.selected_item() else {
            return;
        };
        let id = item.id.clone();
        let existing = item.recurrence.clone();
        let result = parse_due(text)
            .map_err(|err| err.to_string())
            .and_then(|until| {
                let Some(existing) = existing.as_ref() else {
                    return Ok(());
                };
                let recurrence =
                    recurrence_from_choice(recurrence_index(Some(existing)), until, Some(existing));
                self.todos
                    .set_recurrence(&id, recurrence)
                    .map_err(|err| err.to_string())
            });
        self.finish(result, true);
    }
}

fn row_model(row: &Row) -> PanelRow {
    PanelRow {
        title: row.title.as_str().into(),
        completed: row.completed,
        current: row.current,
        due: row.due.as_str().into(),
        overdue: row.overdue,
        recurrence: row.recurrence.into(),
        source: row.source.as_str().into(),
        note: row.trash_note.as_str().into(),
    }
}

#[allow(clippy::cast_precision_loss)]
fn px(value: u32) -> f32 {
    value as f32
}

fn rgb(value: u32) -> slint::Color {
    let [_, r, g, b] = value.to_be_bytes();
    slint::Color::from_rgb_u8(r, g, b)
}
