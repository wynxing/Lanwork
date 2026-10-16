//! 面板窗口：标签框架、待办页和定位接口。
//!
//! 窗口启动时创建并保持隐藏。待办页调用 `TodoCommands`，规则在服务里，这里只把命令结果
//! 排成界面要的行。便签页在 `notes_page`，收纳页在 `shelves_page`，应用页在 `apps_page`，
//! 设置页在 `settings_page`，拖入的 OLE 放下目标登记在这个窗口的句柄上（`shelf_dnd`）。
//! 面板搜索框还没有。
//! 热角在 `hotcorner`，到点调用 [`show`](crate::panel::show)。

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lanwork_core::CivilDate;
use lanwork_core::apps::{LaunchTarget, launch};
use lanwork_core::dispatch::IconCache;
use lanwork_core::notes::NoteCommands;
use lanwork_core::panel::{
    self, DropZone, PanelTab, PanelTarget, RECURRENCE_CHOICES, Row, TodoView, format_date,
    panel_origin, panel_size, parse_due, parse_remind, recurrence_from_choice, recurrence_index,
    reordered_ids,
};
use lanwork_core::shell::{Backdrop, WorkArea, solid_rgb, to_physical};
use lanwork_core::shelves::ShelfCommands;
use lanwork_core::storage::Store;
use lanwork_core::todos::{ListKind, NewTodo, TodoCommands, TodoItem, TodoList};
use slint::winit_030::{EventResult, WinitWindowAccessor, winit};
use slint::{CloseRequestResponse, ComponentHandle, ModelRc, VecModel};

use crate::apps_page::AppsPage;
use crate::bar_win;
use crate::host::{local_today, unix_now_ms};
use crate::keycap;
use crate::notes_page::NotesPage;
use crate::settings_page::{self, SettingsPage, SettingsServices};
use crate::shelf_dnd::{self, DropState, Registration};
use crate::shelves_page::{self, ShelvesPage};
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

/// 应用页的动作。和面板共用同一个借用，所以也走 [`with_panel`]。
pub(crate) fn with_apps(action: impl FnOnce(&mut AppsPage, &Panel) + 'static) {
    with_panel(move |panel| action(&mut panel.apps, &panel.ui));
}

/// 收纳页的动作。和面板共用同一个借用，所以也走 [`with_panel`]。
pub(crate) fn with_shelves(action: impl FnOnce(&mut ShelvesPage, &Panel) + 'static) {
    with_panel(move |panel| action(&mut panel.shelves, &panel.ui));
}

/// 应用页换了分类或打开确认框之后，重新计算放下区域。
pub(crate) fn refresh_drop() {
    with_panel(|panel| panel.sync_drop());
}

/// 搜索条改了隐藏名单之后，设置页如果在，就重画那一节。
pub(crate) fn refresh_hidden_apps() {
    with_settings(|page, ui| page.refresh_hidden(ui));
}

/// 设置页的动作。和面板共用同一个借用，所以也走 [`with_panel`]。
pub(crate) fn with_settings(action: impl FnOnce(&mut SettingsPage, &Panel) + 'static) {
    with_panel(move |panel| action(&mut panel.settings, &panel.ui));
}

/// 启动时调用一次。窗口保持隐藏，直到托盘或搜索结果打开它。
pub(crate) fn install(
    ui: Panel,
    todos: TodoCommands,
    notes: Option<NoteCommands>,
    shelves: Option<ShelfCommands>,
    icons: Arc<Mutex<IconCache>>,
    settings: SettingsServices,
    store: Store,
) {
    let tab_titles: Vec<slint::SharedString> =
        PanelTab::ALL.iter().map(|tab| tab.title().into()).collect();
    ui.set_tab_titles(ModelRc::from(Rc::new(VecModel::from(tab_titles))));
    let lists_model = Rc::new(VecModel::<PanelList>::default());
    let rows_model = Rc::new(VecModel::<PanelRow>::default());
    let triage_actions = Rc::new(VecModel::<slint::SharedString>::default());
    let triage_lists = Rc::new(VecModel::<slint::SharedString>::default());
    ui.set_lists(ModelRc::from(Rc::clone(&lists_model)));
    ui.set_rows(ModelRc::from(Rc::clone(&rows_model)));
    ui.set_triage_actions(ModelRc::from(Rc::clone(&triage_actions)));
    ui.set_triage_lists(ModelRc::from(Rc::clone(&triage_lists)));
    wire(&ui);

    ui.window().on_winit_window_event(|_, event| {
        match event {
            winit::event::WindowEvent::Focused(true) => with_panel(|panel| panel.focused = true),
            winit::event::WindowEvent::Focused(false) => with_panel(Controller::blur),
            _ => {}
        }
        EventResult::Propagate
    });
    // Alt+F4 只收起面板，与关闭按钮相同，不结束进程。
    // 外部 `taskkill`、注销和关机由隐藏的事件目标窗口和热键窗口处理。
    ui.window().on_close_requested(|| {
        with_panel(Controller::hide);
        CloseRequestResponse::KeepWindowShown
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
        shelves: ShelvesPage::new(&ui, shelves, todos.clone(), icons, store.clone()),
        apps: AppsPage::new(&ui, store.clone()),
        settings: SettingsPage::new(&ui, settings, store.clone()),
        drop_state: Arc::new(DropState::default()),
        drop_registration: None,
        linked: HashSet::new(),
        ui,
        todos,
        store,
        lists_model,
        rows_model,
        triage_actions,
        triage_lists,
        session: None,
        cleared_gen: 0,
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
        from_notification: false,
    };
    PANEL.with(|cell| *cell.borrow_mut() = Some(controller));
}

/// 进程退出前调用：录入热键中就结束录入，把热键注册回去。
pub(crate) fn end_hotkey_recording() {
    PANEL.with(|cell| {
        if let Ok(mut slot) = cell.try_borrow_mut()
            && let Some(panel) = slot.as_mut()
        {
            panel.settings.cancel_recording(&panel.ui);
        }
    });
}

/// 托盘「打开面板」「设置」和面板热键。`tab` 为空时保持收起前的标签。
pub(crate) fn show(tab: Option<PanelTab>) {
    with_panel(move |panel| panel.show(tab));
}

/// 打开面板并定位到目标。搜索结果和到期通知都走这里。
pub(crate) fn open_target(target: PanelTarget) {
    with_panel(move |panel| panel.open_target(&target));
}

/// 安装版到期通知的点击。单条定位到该待办；合并补发打开收件箱且不选中。
///
/// 找不到该条时面板上显示「找不到待办」。没拿到前台也不收起，否则错误和定位都看不见。便携版不走这里。
pub(crate) fn open_from_notification(launch: &str) {
    let launch = launch.to_owned();
    with_panel(
        move |panel| match lanwork_core::todos::toast_launch(&launch) {
            Some(lanwork_core::todos::ToastLaunch::Item(id)) => panel.open_notification(&id),
            Some(lanwork_core::todos::ToastLaunch::Inbox) => panel.open_notification_inbox(),
            None => {}
        },
    );
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
            if panel.tab == PanelTab::Shelves {
                panel.shelves.reload(&panel.ui);
            }
        }
    });
}

/// 收纳有变更消息时调用。分组变了，待办页的「收纳」标记和「打开关联收纳」也要跟着变，
/// 不论当前停在哪个标签，所以待办页的数据总是重排。面板隐藏时不重画，下次显示再读。
pub(crate) fn on_shelves_changed() {
    with_panel(|panel| {
        if !panel.visible {
            return;
        }
        if panel.tab == PanelTab::Shelves {
            panel.shelves.reload(&panel.ui);
        }
        panel.reload();
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
    ui.on_open_shelf(|| with_panel(Controller::open_shelf));
    ui.on_float_todos(|| with_panel(Controller::float_todos));
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
    ui.on_start_triage(|| with_panel(Controller::enter_triage));
    ui.on_triage_key(|code, modified| {
        let handled = Rc::new(Cell::new(false));
        let flag = Rc::clone(&handled);
        with_panel(move |panel| flag.set(panel.on_triage_key(code, modified)));
        handled.get()
    });
    ui.on_triage_click(|index| with_panel(move |panel| panel.triage_click(index)));
    ui.on_triage_pick(|index| with_panel(move |panel| panel.triage_pick(index)));
}

pub(crate) struct Controller {
    ui: Panel,
    notes: NotesPage,
    shelves: ShelvesPage,
    apps: AppsPage,
    settings: SettingsPage,
    drop_state: Arc<DropState>,
    drop_registration: Option<Registration>,
    /// 被收纳分组关联的待办 id。待办页的标记和「打开关联收纳」用它。
    linked: HashSet<String>,
    todos: TodoCommands,
    store: Store,
    lists_model: Rc<VecModel<PanelList>>,
    rows_model: Rc<VecModel<PanelRow>>,
    triage_actions: Rc<VecModel<slint::SharedString>>,
    triage_lists: Rc<VecModel<slint::SharedString>>,
    /// 处理模式这一轮。没有时待办页是清单。
    session: Option<panel::Session>,
    /// 作废已经排好的「1 秒后回到清单」。
    cleared_gen: u64,
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
    /// 这次显示来自到期通知。前台失败时不收起，并再显示一次以盖过 `SW_HIDE`。
    from_notification: bool,
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
        let capture_ready = self
            .hwnd()
            .is_some_and(|hwnd| keycap::install(hwnd, settings_page::on_recorded));
        self.settings.set_capture_ready(capture_ready);
        if !capture_ready {
            self.store
                .log_warn("面板按键子类没有装上，设置页不能录入热键");
        }
        if let Some(hwnd) = self.hwnd() {
            let state = Arc::clone(&self.drop_state);
            match shelf_dnd::register(
                hwnd,
                Arc::clone(&self.drop_state),
                Arc::new(move |incoming| {
                    if state.files_only() {
                        crate::apps_page::dropped(incoming);
                    } else {
                        shelves_page::dropped(incoming);
                    }
                }),
            ) {
                Ok(registration) => self.drop_registration = Some(registration),
                Err(message) => self
                    .store
                    .log_warn(&format!("收纳页拖入没有装上，不能拖入文件：{message}")),
            }
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

    fn open_notification(&mut self, item_id: &str) {
        self.from_notification = true;
        self.open_target(&PanelTarget::Todo {
            item_id: item_id.to_owned(),
        });
        self.from_notification = false;
    }

    /// 合并补发：待办标签、收件箱，不选中任何一条。
    fn open_notification_inbox(&mut self) {
        self.from_notification = true;
        self.exit_triage();
        self.tab = PanelTab::Todo;
        self.prepare();
        self.view = panel::default_view(&self.lists);
        self.selected = None;
        self.rebuild();
        self.present();
        self.from_notification = false;
    }

    fn open_target(&mut self, target: &PanelTarget) {
        self.exit_triage();
        self.tab = match target {
            PanelTarget::Todo { .. } => PanelTab::Todo,
            PanelTarget::Note { .. } => PanelTab::Notes,
            PanelTarget::Shelf { .. } => PanelTab::Shelves,
        };
        self.prepare();
        match target {
            PanelTarget::Todo { item_id } => self.locate(item_id),
            PanelTarget::Note { note_id } => self.notes.locate(&self.ui, note_id),
            PanelTarget::Shelf { group_id } => self.shelves.locate(&self.ui, group_id),
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
        self.push_error();
        if self.tab == PanelTab::Notes {
            self.notes.entered(&self.ui);
        }
        if self.tab == PanelTab::Shelves {
            self.shelves.entered(&self.ui);
        }
        if self.tab == PanelTab::Apps {
            self.apps.entered(&self.ui);
        }
        if self.tab == PanelTab::Settings {
            self.settings.entered(&self.ui);
        } else {
            self.settings.cancel_recording(&self.ui);
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
        let hwnd = self.hwnd();
        if let Some(hwnd) = hwnd
            && bar_win::window_origin(hwnd) != Some(origin)
            && !self.focused
        {
            bar_win::move_window(hwnd, origin.0, origin.1);
        }
        if self.from_notification
            && let Some(hwnd) = hwnd
        {
            bar_win::reveal_activated_window(hwnd);
        }
        // 没拿到前台就收起。visible 已经是真，但失焦收起还要求曾经拿到焦点；
        // 焦点没到时会停在已显示，下一次热角再 show 也出不来。规格没写这种失败，待定。
        // 到期通知是用户点开来的，前台失败时仍留着，否则定位和「找不到待办」都看不见。
        match hwnd.map(bar_win::bring_to_front) {
            Some(bar_win::ForegroundTake::Plain) => {}
            Some(bar_win::ForegroundTake::AfterInput) => {
                self.store.log_info("面板在注入输入后到前台");
            }
            Some(bar_win::ForegroundTake::Failed) | None => {
                if self.from_notification {
                    self.store.log_warn("到期通知已打开面板，但没有到前台");
                    return;
                }
                self.store.log_warn(if hwnd.is_none() {
                    "面板没有窗口句柄，已收起"
                } else {
                    "面板没有到前台，已收起"
                });
                self.focused = false;
                self.hide();
                return;
            }
        }
        self.ui.invoke_focus_root();
        self.ui.invoke_ensure_row_visible(self.selected_index());
        self.sync_drop();
    }

    /// 收纳标签接受文件和网址。应用页在没有确认框时只接受一个文件夹或 zip。
    fn sync_drop(&self) {
        let shelves = self.visible && self.tab == PanelTab::Shelves;
        let apps = self.visible && self.tab == PanelTab::Apps && self.apps.accepts_drop();
        self.drop_state.set_files_only(apps);
        self.drop_state.set_accepting(shelves || apps);
        self.drop_state.set_zone(DropZone {
            x: self.ui.get_shelf_zone_x(),
            y: self.ui.get_shelf_zone_y(),
            width: self.ui.get_shelf_zone_w(),
            height: self.ui.get_shelf_zone_h(),
            scale: self.ui.window().scale_factor(),
        });
    }

    fn hide(&mut self) {
        self.exit_triage();
        self.settings.cancel_recording(&self.ui);
        self.apps.dismiss(&self.ui);
        self.visible = false;
        self.error = None;
        self.sync_drop();
        let _ = self.ui.hide();
    }

    fn blur(&mut self) {
        // 录入中失焦，哪怕在刚显示的宽限期内，也要结束录入并把热键注册回去。
        self.settings.cancel_recording(&self.ui);
        let settling = self
            .shown_at
            .is_some_and(|shown| shown.elapsed() < BLUR_GRACE);
        if self.visible && self.focused && !settling && !self.shelves.is_dragging() {
            self.hide();
        }
    }

    fn drag(&mut self) {
        let _ = self.ui.window().with_winit_window(|window| {
            let _ = window.drag_window();
        });
    }

    fn select_tab(&mut self, tab: PanelTab) {
        if tab != PanelTab::Todo {
            self.exit_triage();
        }
        self.tab = tab;
        self.ui.set_tab(i32::try_from(tab.index()).unwrap_or(0));
        if tab != PanelTab::Settings {
            self.settings.cancel_recording(&self.ui);
        }
        if tab != PanelTab::Apps {
            self.apps.dismiss(&self.ui);
        }
        if tab == PanelTab::Notes {
            self.notes.entered(&self.ui);
        } else if tab == PanelTab::Shelves {
            self.shelves.entered(&self.ui);
        } else if tab == PanelTab::Apps {
            self.error = None;
            self.push_error();
            self.apps.entered(&self.ui);
        } else if tab == PanelTab::Settings {
            self.error = None;
            self.push_error();
            self.settings.entered(&self.ui);
        } else {
            self.reload();
            self.push_error();
        }
        self.sync_drop();
        if self.session.is_some() {
            self.ui.invoke_focus_triage();
        } else {
            self.ui.invoke_focus_root();
        }
    }

    fn today(&mut self) -> Option<CivilDate> {
        let today = local_today();
        if today.is_none() {
            self.store.log_warn("读不到本地日期，面板不标逾期");
        }
        today
    }

    /// 从服务重新读清单，保持当前视图和选中的条目，然后推给界面。
    /// 处理模式开着时，按条目 id 跟上队列，不把光标拉回去。
    fn reload(&mut self) {
        if self.reload_lists() {
            self.follow_triage();
        }
    }

    /// 重新读清单并重排。不移动处理模式的光标。清单没读到时返回假。
    fn reload_lists(&mut self) -> bool {
        match self.todos.lists() {
            Ok(lists) => self.lists = lists,
            Err(err) => {
                self.error = Some(err.to_string());
                self.push_error();
                return false;
            }
        }
        let keep = self
            .view
            .clone()
            .filter(|view| panel::view_exists(&self.lists, view));
        self.view = keep.or_else(|| panel::default_view(&self.lists));
        self.rebuild();
        true
    }

    /// 由 `lists` 和 `view` 重排界面数据，不读服务。
    fn rebuild(&mut self) {
        let today = self.today();
        let now = unix_now_ms();
        self.linked = self.shelves.linked_todos();
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
        let models: Vec<PanelRow> = self
            .rows
            .iter()
            .map(|row| row_model(row, self.linked.contains(&row.id)))
            .collect();
        self.rows_model.set_vec(models);
        self.ui.set_selected_row(self.selected_index());
        let incomplete = panel::inbox_queue(&self.lists).len();
        self.ui.set_show_process(
            panel::entry_visible(self.viewing_inbox(), incomplete) && self.session.is_none(),
        );
        self.ui
            .set_process_label(panel::entry_label(incomplete).into());
        self.push_detail(false);
    }

    fn viewing_inbox(&self) -> bool {
        let Some(TodoView::List(id)) = &self.view else {
            return false;
        };
        self.lists
            .iter()
            .any(|list| &list.id == id && list.kind == ListKind::Inbox)
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
            has_shelf: self.linked.contains(&item.id),
            current: item.current,
            can_up: reordered_ids(&self.lists, &list.id, &item.id, true).is_some(),
            can_down: reordered_ids(&self.lists, &list.id, &item.id, false).is_some(),
        };
        self.ui.set_detail(detail);
    }

    /// 面板自己的错误优先；没有时显示设置页留着的热键没注册上的提示，它收起时不清。
    fn push_error(&mut self) {
        let text = self
            .error
            .clone()
            .or_else(|| self.settings.hotkey_error())
            .unwrap_or_default();
        self.ui.set_error(text.into());
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

    /// 待办页的「悬浮」：打开全程序唯一的待办悬浮窗。已经打开时只把焦点给它。
    fn float_todos(&mut self) {
        self.finish(crate::todo_float::open(), false);
    }

    /// 「打开关联收纳」：切到收纳并选中关联这条待办的分组，关联多个时选第一个。
    fn open_shelf(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let Some(group_id) = self.shelves.group_for_todo(&id) else {
            self.error = Some("没有关联的收纳分组".to_owned());
            self.push_error();
            return;
        };
        self.open_target(&PanelTarget::Shelf { group_id });
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

    fn enter_triage(&mut self) {
        if self.session.is_some() || !self.viewing_inbox() {
            return;
        }
        let ids = panel::inbox_queue(&self.lists);
        self.session = Some(panel::Session::start(ids));
        self.error = None;
        self.rebuild();
        self.push_error();
        self.push_triage();
        self.ui.invoke_focus_triage();
    }

    fn exit_triage(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.session = None;
        self.cleared_gen = self.cleared_gen.wrapping_add(1);
        self.error = None;
        if let Some(inbox) = self.lists.iter().find(|list| list.kind == ListKind::Inbox) {
            self.view = Some(TodoView::List(inbox.id.clone()));
        }
        self.selected = None;
        self.rebuild();
        self.push_triage();
        self.push_error();
        self.ui.invoke_scroll_to_top();
        self.ui.invoke_focus_root();
    }

    fn on_triage_key(&mut self, code: i32, modified: bool) -> bool {
        let Some((picker_open, showing_item)) = self
            .session
            .as_ref()
            .map(|session| (session.picker_open(), session.showing_item()))
        else {
            return false;
        };
        let hit = panel::key_hit(panel::KeyInput {
            code,
            composing: false,
            modified,
            picker_open,
            showing_item,
        });
        if hit.action != panel::Action::None {
            self.apply_triage(hit.action);
        }
        hit.consume
    }

    fn triage_click(&mut self, index: i32) {
        let action = match index {
            0 => panel::Action::Today,
            1 => panel::Action::Defer,
            2 => panel::Action::OpenMove,
            3 => panel::Action::Complete,
            4 => panel::Action::Delete,
            _ => return,
        };
        self.apply_triage(action);
    }

    fn triage_pick(&mut self, index: i32) {
        let Some(index) = usize::try_from(index).ok() else {
            return;
        };
        let target = self
            .session
            .as_mut()
            .and_then(|session| session.take_pick(index));
        if let Some(target) = target {
            self.move_current(&target);
        }
    }

    fn apply_triage(&mut self, action: panel::Action) {
        match action {
            panel::Action::Exit => self.exit_triage(),
            panel::Action::CancelMove => {
                if let Some(session) = self.session.as_mut() {
                    session.close_picker();
                }
                self.push_triage();
                self.ui.invoke_focus_triage();
            }
            panel::Action::MoveUp => self.nudge_picker(false),
            panel::Action::MoveDown => self.nudge_picker(true),
            panel::Action::ConfirmMove => {
                let target = self
                    .session
                    .as_mut()
                    .and_then(|session| session.take_current_pick());
                if let Some(target) = target {
                    self.move_current(&target);
                }
            }
            panel::Action::OpenMove => self.open_move(),
            panel::Action::Today => self.write_current(TriageWrite::Today),
            panel::Action::Defer => self.write_current(TriageWrite::Defer),
            panel::Action::Complete => self.write_current(TriageWrite::Complete),
            panel::Action::Delete => self.write_current(TriageWrite::Delete),
            panel::Action::None => {}
        }
    }

    fn nudge_picker(&mut self, down: bool) {
        if let Some(session) = self.session.as_mut() {
            session.move_picker(down);
        }
        self.push_triage();
        self.ui.invoke_focus_triage();
    }

    fn open_move(&mut self) {
        let opened = {
            let Some(session) = self.session.as_mut() else {
                return;
            };
            if session.picker_open() || !session.showing_item() {
                return;
            }
            session.open_picker(&self.lists)
        };
        match opened {
            panel::OpenMove::Opened => {
                self.push_triage();
                self.ui.invoke_focus_triage();
            }
            panel::OpenMove::NoList => {
                self.error = Some(panel::NO_OTHER_LIST.to_owned());
                self.push_error();
                self.ui.invoke_focus_triage();
            }
            panel::OpenMove::Unavailable => {}
        }
    }

    fn move_current(&mut self, target_id: &str) {
        let Some(id) = self
            .session
            .as_ref()
            .and_then(|session| session.shown_id().map(str::to_owned))
        else {
            return;
        };
        let result = self
            .todos
            .process_move(&id, target_id)
            .map_err(|err| err.to_string());
        self.finish_triage(result, &id);
    }

    fn write_current(&mut self, write: TriageWrite) {
        let Some(id) = self
            .session
            .as_ref()
            .and_then(|session| session.shown_id().map(str::to_owned))
        else {
            return;
        };
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.picker_open())
        {
            return;
        }
        let result = match write {
            TriageWrite::Today => match self.today() {
                Some(today) => self
                    .todos
                    .process_today(&id, today)
                    .map_err(|err| err.to_string()),
                None => Err(panel::NO_TODAY.to_owned()),
            },
            TriageWrite::Defer => match self.today() {
                Some(today) => self
                    .todos
                    .process_defer(&id, self.settings.process_snooze_days(), today)
                    .map_err(|err| err.to_string()),
                None => Err(panel::NO_TODAY.to_owned()),
            },
            TriageWrite::Complete => self
                .todos
                .process_complete(&id)
                .map_err(|err| err.to_string()),
            TriageWrite::Delete => self
                .todos
                .process_soft_delete(&id)
                .map_err(|err| err.to_string()),
        };
        self.finish_triage(result, &id);
    }

    fn finish_triage(&mut self, result: Result<(), String>, id: &str) {
        match result {
            Ok(()) => {
                self.error = None;
                self.push_error();
                if !self.reload_lists() {
                    self.push_triage();
                    self.ui.invoke_focus_triage();
                    return;
                }
                match self.todos.next_inbox_incomplete_after(id) {
                    Ok(next) => self.step_cursor(next),
                    Err(err) => {
                        self.error = Some(err.to_string());
                        self.push_error();
                        self.push_triage();
                        self.ui.invoke_focus_triage();
                    }
                }
            }
            Err(message) => {
                self.error = Some(message);
                let _ = self.reload_lists();
                self.push_error();
                self.push_triage();
                self.ui.invoke_focus_triage();
            }
        }
    }

    fn step_cursor(&mut self, next: Option<String>) {
        let was_cleared = self
            .session
            .as_ref()
            .is_some_and(|session| session.is_cleared());
        if let Some(session) = self.session.as_mut() {
            session.advance(next);
        }
        let cleared = self
            .session
            .as_ref()
            .is_some_and(|session| session.is_cleared());
        if cleared && !was_cleared {
            self.arm_cleared();
        }
        self.push_triage();
        self.ui.invoke_focus_triage();
    }

    fn arm_cleared(&mut self) {
        self.cleared_gen = self.cleared_gen.wrapping_add(1);
        let generation = self.cleared_gen;
        slint::Timer::single_shot(Duration::from_millis(panel::CLEARED_HOLD_MS), move || {
            with_panel(move |panel| panel.return_if_cleared(generation));
        });
    }

    fn return_if_cleared(&mut self, generation: u64) {
        if self.cleared_gen == generation
            && self
                .session
                .as_ref()
                .is_some_and(|session| session.is_cleared())
        {
            self.exit_triage();
        }
    }

    fn follow_triage(&mut self) {
        if self.session.is_none() {
            return;
        }
        let fresh = panel::inbox_queue(&self.lists);
        let Some((cleared, empty, showing, shown_in, cursor)) =
            self.session.as_ref().map(|session| {
                (
                    session.is_cleared(),
                    session.is_empty(),
                    session.showing_item(),
                    session.shown_in(&fresh),
                    session.cursor().to_owned(),
                )
            })
        else {
            return;
        };
        if empty {
            if !fresh.is_empty() {
                self.session = Some(panel::Session::start(fresh));
            }
            self.push_triage();
            return;
        }
        if showing && shown_in {
            if let Some(session) = self.session.as_mut() {
                session.absorb_ahead(&fresh);
            }
            self.push_triage();
            return;
        }
        if showing && !shown_in {
            match self.todos.next_inbox_incomplete_after(&cursor) {
                Ok(next) => self.step_cursor(next),
                Err(err) => {
                    self.error = Some(err.to_string());
                    self.push_error();
                    self.push_triage();
                }
            }
            return;
        }
        if cleared {
            match self.todos.next_inbox_incomplete_after(&cursor) {
                Ok(Some(next)) => {
                    self.cleared_gen = self.cleared_gen.wrapping_add(1);
                    self.step_cursor(Some(next));
                }
                Ok(None) => self.push_triage(),
                Err(err) => {
                    self.error = Some(err.to_string());
                    self.push_error();
                }
            }
        }
    }

    fn push_triage(&mut self) {
        let Some(view) = self.triage_view() else {
            self.ui.set_triage_open(false);
            self.ui.set_triage_show_card(false);
            self.ui.set_triage_picker(false);
            return;
        };
        self.ui.set_triage_open(true);
        self.ui.set_triage_show_card(view.show_card);
        self.ui.set_triage_status(view.status.into());
        self.ui.set_triage_progress(view.progress.into());
        self.ui.set_triage_title(view.title.into());
        self.ui.set_triage_due(view.due.into());
        self.ui.set_triage_source(view.source.into());
        self.triage_actions
            .set_vec(view.labels.into_iter().map(Into::into).collect::<Vec<_>>());
        self.ui.set_triage_picker(view.picker);
        self.triage_lists
            .set_vec(view.names.into_iter().map(Into::into).collect::<Vec<_>>());
        self.ui.set_triage_list_index(view.index);
    }

    fn triage_view(&self) -> Option<TriageView> {
        let session = self.session.as_ref()?;
        let card = session
            .shown_id()
            .and_then(|id| panel::card_text(&self.lists, id))
            .unwrap_or(panel::CardText {
                title: String::new(),
                due: String::new(),
                source: String::new(),
            });
        Some(TriageView {
            show_card: session.showing_item(),
            status: session.status().unwrap_or("").to_owned(),
            progress: session.progress().unwrap_or_default(),
            title: card.title,
            due: card.due,
            source: card.source,
            labels: panel::action_labels(self.settings.process_snooze_days()),
            picker: session.picker_open(),
            names: session.picker_names(),
            index: i32::try_from(session.picker_index()).unwrap_or(0),
        })
    }
}

struct TriageView {
    show_card: bool,
    status: String,
    progress: String,
    title: String,
    due: String,
    source: String,
    labels: [String; 5],
    picker: bool,
    names: Vec<String>,
    index: i32,
}

enum TriageWrite {
    Today,
    Defer,
    Complete,
    Delete,
}

fn row_model(row: &Row, shelf: bool) -> PanelRow {
    PanelRow {
        title: row.title.as_str().into(),
        completed: row.completed,
        current: row.current,
        due: row.due.as_str().into(),
        overdue: row.overdue,
        recurrence: row.recurrence.into(),
        source: row.source.as_str().into(),
        note: row.trash_note.as_str().into(),
        shelf,
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
