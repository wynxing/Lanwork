//! 便签和收纳悬浮窗的叠页。几何在 `lanwork_core::panel::float_stack`，这里只接窗口。
//!
//! 一叠同一时刻只显示一页。其余页的窗口藏着，不释放。彩色圆点挂在边框外，与窗口左边缘对齐。
//! 关掉窗口不拆叠，成员还在文件里；再悬浮时用记下的当前页和贴边。
//! 启动时按文件里的 `floatStack` 合成一叠，只打开当前页。
//! 下面没写进产品规格的做法都待定。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lanwork_core::notes::{FloatGeometry, FloatStackSlot, NoteCommands};
use lanwork_core::panel::{
    DockState, DragOrigin, DropEffect, DropInput, Flip, OtherFloat, PageStack, PxBox, StackMember,
    StackPage, StartupPage, assemble_stacks, collapsed_handles_close, detach_page, dock_merge_gap,
    dot_row_size, dpi_at, filter_open_pages, fit_dot_row, flip_index, handle_dot_count,
    merge_stacks, moved_enough, pages_to_park, plan_startup, restore_log_line, snap_on_release,
    stack_drop_effect,
};
use lanwork_core::shelves::ShelfCommands;
use lanwork_core::storage::Store;
use lanwork_core::todos::{FloatWindowFile, TodoCommands};
use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;

use crate::float_dock::{self, SavedFloat};
use crate::{StackTab, note_float, shelf_float, todo_float};

struct Tracked {
    saved: SavedFloat,
    visible: PxBox,
}

struct TabDrag {
    page_at: PxBox,
    tab_at: (i32, i32),
    /// 按下的是哪一页。不一定是当时显示的那一页。
    page_index: usize,
}

struct PageTab {
    ui: StackTab,
    key: Option<isize>,
    drag: Option<TabDrag>,
}

struct Live {
    notes: Option<NoteCommands>,
    shelves: Option<ShelfCommands>,
    todos: TodoCommands,
    store: Store,
    stacks: HashMap<String, PageStack>,
    member: HashMap<StackPage, String>,
    shown: HashMap<StackPage, Tracked>,
    tabs: HashMap<String, PageTab>,
    dark: bool,
    merge_depth: u8,
}

thread_local! {
    static LIVE: RefCell<Option<Live>> = const { RefCell::new(None) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn install(
    notes: Option<NoteCommands>,
    shelves: Option<ShelfCommands>,
    todos: TodoCommands,
    store: Store,
) {
    crate::bar_win::add_move_hook(on_tab_move);
    crate::float_frame::set_log(store.clone());
    LIVE.with(|cell| {
        *cell.borrow_mut() = Some(Live {
            notes,
            shelves,
            todos,
            store,
            stacks: HashMap::new(),
            member: HashMap::new(),
            shown: HashMap::new(),
            tabs: HashMap::new(),
            dark: false,
            merge_depth: 0,
        });
    });
    // 等事件循环起来再开窗口，避免 show 发生在循环之前。
    later(|| with_live(Live::restore_saved));
}

pub(crate) fn set_dark(dark: bool) {
    with_live(move |live| {
        live.dark = dark;
        for tab in live.tabs.values() {
            tab.ui.set_dark(dark);
        }
    });
}

pub(crate) fn page_opened(page: StackPage) {
    if BUSY.get() {
        return;
    }
    BUSY.set(true);
    with_live(|live| live.on_opened(page));
    BUSY.set(false);
}

pub(crate) fn page_moved(page: StackPage) {
    with_live(|live| live.on_moved(page));
}

pub(crate) fn page_closed(page: StackPage) {
    with_live(|live| {
        live.shown.remove(&page);
        live.sync_tabs();
    });
}

pub(crate) fn flip(page: StackPage, dir: Flip) {
    with_live(|live| live.flip(page, dir));
}

/// 标题栏松手。只读，不改窗口。
pub(crate) fn classify_title(
    page: StackPage,
    anchor: PxBox,
    released: PxBox,
    from_collapsed: bool,
) -> DropEffect {
    LIVE.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|live| {
                live.classify(
                    &page,
                    DragOrigin::TitleBar,
                    anchor,
                    released,
                    from_collapsed,
                )
            })
            .unwrap_or(DropEffect::MoveTogether)
    })
}

pub(crate) fn queue_detach(page: StackPage, before: SavedFloat, detached: SavedFloat) {
    later(move || {
        with_live(|live| live.detach_title(page, before, detached));
    });
}

pub(crate) fn queue_merge_unit(dragged: StackPage, target: StackPage) {
    later(move || {
        with_live(|live| live.merge_unit(dragged, target));
    });
}

fn with_live(action: impl FnOnce(&mut Live)) {
    LIVE.with(|cell| {
        if let Some(live) = cell.borrow_mut().as_mut() {
            action(live);
        }
    });
}

fn later(action: impl FnOnce() + 'static) {
    slint::Timer::single_shot(Duration::ZERO, action);
}

fn new_stack_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(1);
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("s{millis:x}{seq:x}")
}

fn saved_from(expanded: PxBox, dock: Option<lanwork_core::notes::FloatDock>) -> SavedFloat {
    SavedFloat {
        geometry: FloatGeometry {
            x: expanded.x,
            y: expanded.y,
            width: expanded.width,
            height: expanded.height,
        },
        dock,
    }
}

fn shift(rect: PxBox, dx: i32, dy: i32) -> PxBox {
    PxBox {
        x: rect.x.saturating_add(dx),
        y: rect.y.saturating_add(dy),
        width: rect.width,
        height: rect.height,
    }
}

impl Live {
    fn classify(
        &self,
        page: &StackPage,
        origin: DragOrigin,
        anchor: PxBox,
        released: PxBox,
        from_collapsed: bool,
    ) -> DropEffect {
        let screens = float_dock::screens();
        let dpi = dpi_at(&screens, released.x, released.y);
        let moved = moved_enough((anchor.x, anchor.y), (released.x, released.y), dpi);
        stack_drop_effect(&DropInput {
            origin,
            start: anchor,
            released,
            moved,
            from_collapsed,
            page_count: self.page_count(page),
            snapped: snap_on_release(released, &screens).is_some(),
            others: self.others(page),
        })
    }

    fn page_count(&self, page: &StackPage) -> usize {
        self.stack_of(page)
            .map(|stack| stack.pages.len())
            .unwrap_or(1)
    }

    fn stack_of(&self, page: &StackPage) -> Option<&PageStack> {
        let id = self.member.get(page)?;
        self.stacks.get(id)
    }

    fn others(&self, page: &StackPage) -> Vec<OtherFloat> {
        self.shown
            .iter()
            .filter(|(other, _)| *other != page && !self.same_stack(page, other))
            .map(|(other, tracked)| OtherFloat {
                page: (*other).clone(),
                rect: tracked.visible,
            })
            .collect()
    }

    fn on_opened(&mut self, page: StackPage) {
        self.ensure_loaded(&page);
        if self.member.contains_key(&page) {
            // 启动还原和翻页会再进来。藏着的页没有窗口，不要把当前页换成它。
            if self.read_view(&page).is_none() {
                return;
            }
            let Some(mut stack) = self.take_stack(&page) else {
                return;
            };
            self.retain_open(&mut stack);
            if let Some(index) = stack.pages.iter().position(|item| item == &page) {
                stack.current = index;
            }
            let saved = self
                .saved_of_shown(&stack)
                .or_else(|| self.disk_saved(&page))
                .or_else(|| self.read_view(&page).map(|(saved, _)| saved));
            if self.finish_thin(&stack, saved) {
                self.track(&page);
                self.sync_tabs();
                return;
            }
            self.store_stack(&stack);
            if let Some(saved) = saved {
                self.persist(&stack, saved);
                let _ = self.reveal(&stack, saved);
            }
            self.sync_tabs();
        } else {
            self.paint_chrome(&page, 1, 0);
            self.track(&page);
            self.sync_tabs();
        }
    }

    fn on_moved(&mut self, page: StackPage) {
        self.track(&page);
        if let Some(stack) = self.stack_of(&page).cloned()
            && stack.pages.get(stack.current) == Some(&page)
            && let Some(tracked) = self.shown.get(&page)
        {
            let saved = tracked.saved;
            self.persist(&stack, saved);
        }
        self.sync_tabs();
        self.consider_dock_merge(&page);
    }

    fn flip(&mut self, page: StackPage, dir: Flip) {
        self.ensure_loaded(&page);
        let next = self
            .stack_of(&page)
            .and_then(|stack| flip_index(stack, dir));
        self.reveal_index(page, next);
    }

    /// 点页点时换到那一页。已经是当前页就只把点的位置摆回去。
    fn activate_index(&mut self, page: StackPage, index: usize) {
        self.ensure_loaded(&page);
        if self
            .stack_of(&page)
            .is_some_and(|stack| stack.current == index)
        {
            self.sync_tabs();
            return;
        }
        self.reveal_index(page, Some(index));
    }

    /// `next` 为 `None` 时停在当前页（翻到头）。下标越界同样停住。
    fn reveal_index(&mut self, page: StackPage, next: Option<usize>) {
        let Some(mut stack) = self.take_stack(&page) else {
            return;
        };
        self.retain_open(&mut stack);
        let saved = self
            .read_view(&page)
            .map(|(saved, _)| saved)
            .or_else(|| self.disk_saved(&page));
        if self.finish_thin(&stack, saved) {
            self.sync_tabs();
            return;
        }
        let next = next.filter(|index| *index < stack.pages.len() && *index != stack.current);
        let Some(next) = next else {
            self.store_stack(&stack);
            if let Some(saved) = saved {
                self.persist(&stack, saved);
                self.paint_current(&stack);
            }
            self.sync_tabs();
            return;
        };
        let Some(saved) = saved else {
            self.store_stack(&stack);
            return;
        };
        let target = stack.pages[next].clone();
        stack.current = next;
        self.store_stack(&stack);
        let count = i32::try_from(stack.pages.len()).unwrap_or(i32::MAX);
        let index = i32::try_from(next).unwrap_or(0);
        if !self.show_page(&target, saved, count, index) {
            self.abandon_failed_show(&page, &target, saved);
            self.sync_tabs();
            return;
        }
        self.persist(&stack, saved);
        for other in pages_to_park(&stack.pages, &target, true) {
            self.park_page(&other);
        }
        self.sync_tabs();
    }

    fn detach_title(&mut self, page: StackPage, before: SavedFloat, detached: SavedFloat) {
        self.split_off(&page, before);
        self.write_solo(&page, detached);
        self.paint_chrome(&page, 1, 0);
        self.track(&page);
        self.sync_tabs();
    }

    fn merge_unit(&mut self, dragged: StackPage, target: StackPage) {
        if self.merge_depth > 4 {
            return;
        }
        self.merge_depth += 1;
        let target_stack = self
            .take_stack(&target)
            .unwrap_or_else(|| solo_stack(&target));
        let dragged_stack = if target_stack.pages.contains(&dragged) {
            solo_stack(&dragged)
        } else {
            self.take_stack(&dragged)
                .unwrap_or_else(|| solo_stack(&dragged))
        };
        let stay = self
            .shown
            .get(&target)
            .map(|tracked| tracked.saved)
            .or_else(|| self.disk_saved(&target))
            .or_else(|| self.disk_saved(&dragged));
        let Some(stay) = stay else {
            self.store_stack(&target_stack);
            if !target_stack.pages.contains(&dragged) {
                self.store_stack(&dragged_stack);
            }
            self.merge_depth = self.merge_depth.saturating_sub(1);
            return;
        };
        let merged = merge_stacks(target_stack, dragged_stack);
        if merged.pages.len() < 2 {
            self.store_stack(&merged);
            self.merge_depth = self.merge_depth.saturating_sub(1);
            return;
        }
        self.store_stack(&merged);
        self.persist(&merged, stay);
        let current = merged.pages[merged.current].clone();
        if self.reveal(&merged, stay) {
            self.consider_dock_merge(&current);
        }
        self.sync_tabs();
        self.merge_depth = self.merge_depth.saturating_sub(1);
    }

    fn split_off(&mut self, page: &StackPage, stay: SavedFloat) {
        let Some(stack) = self.take_stack(page) else {
            return;
        };
        let index = stack
            .pages
            .iter()
            .position(|item| item == page)
            .unwrap_or(stack.current);
        let outcome = detach_page(&stack, index);
        if let Some(rest) = outcome.remaining {
            self.store_stack(&rest);
            self.persist(&rest, stay);
            let _ = self.reveal(&rest, stay);
        } else if let Some(left) = outcome.leftover {
            self.write_solo(&left, stay);
            let _ = self.show_page(&left, stay, 1, 0);
        }
    }

    fn consider_dock_merge(&mut self, page: &StackPage) {
        if self.merge_depth > 4 {
            return;
        }
        let Some((saved, visible)) = self
            .shown
            .get(page)
            .map(|tracked| (tracked.saved, tracked.visible))
        else {
            return;
        };
        let Some(dock) = saved.dock else {
            return;
        };
        if !dock.collapsed {
            return;
        }
        let screens = float_dock::screens();
        let dpi = dpi_at(&screens, visible.x, visible.y);
        let gap = dock_merge_gap(dpi);
        let Some(target) = self.shown.iter().find_map(|(other, place)| {
            if other == page {
                return None;
            }
            if self.same_stack(page, other) {
                return None;
            }
            let other_dock = place.saved.dock?;
            if !other_dock.collapsed {
                return None;
            }
            collapsed_handles_close(
                visible,
                dock.edge,
                place.visible,
                other_dock.edge,
                gap,
                &screens,
            )
            .then(|| (*other).clone())
        }) else {
            return;
        };
        self.merge_unit(page.clone(), target);
    }

    fn same_stack(&self, left: &StackPage, right: &StackPage) -> bool {
        match (self.member.get(left), self.member.get(right)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }

    /// 启动时把记过叠页的悬浮窗合成一叠，只打开当前页。单独一页的不打开。
    fn restore_saved(&mut self) {
        if self.notes.is_none() {
            self.store.log_warn("便签服务没有打开，叠页还原看不到便签");
        }
        if self.shelves.is_none() {
            self.store.log_warn("收纳服务没有打开，叠页还原看不到收纳");
        }
        let Some(pages) = self.startup_pages() else {
            return;
        };
        for item in plan_startup(&pages) {
            self.clear_trashed_slots(&item.stack.id);
            let saved = SavedFloat {
                geometry: item.geometry,
                dock: item.dock,
            };
            self.store_stack(&item.stack);
            if self.reveal(&item.stack, saved) {
                self.persist(&item.stack, saved);
                self.store.log_info(&restore_log_line(
                    &item.stack.id,
                    item.stack.pages.len(),
                    item.stack.current,
                ));
            } else {
                self.store
                    .log_warn(&format!("叠页没有还原：{}", item.stack.id));
            }
        }
        self.sync_tabs();
    }

    fn startup_pages(&self) -> Option<Vec<StartupPage>> {
        let mut pages = Vec::new();
        if let Some(notes) = &self.notes {
            for note in notes.list() {
                if let Some(slot) = note.float_stack {
                    pages.push(StartupPage {
                        page: StackPage::Note(note.id),
                        slot,
                        geometry: note.float,
                        dock: note.float_dock,
                    });
                }
            }
        }
        if let Some(shelves) = &self.shelves {
            match shelves.list() {
                Ok(groups) => {
                    for group in groups {
                        if let Some(slot) = group.float_stack {
                            pages.push(StartupPage {
                                page: StackPage::Shelf(group.id),
                                slot,
                                geometry: group.float,
                                dock: group.float_dock,
                            });
                        }
                    }
                }
                Err(err) => {
                    self.store
                        .log_warn(&format!("叠页没有还原，收纳列表读不出来：{err}"));
                    return None;
                }
            }
        }
        match self.todos.read_float_window() {
            Ok(FloatWindowFile::Record(record)) => {
                if let Some(slot) = record.stack {
                    pages.push(StartupPage {
                        page: StackPage::Todo,
                        slot,
                        geometry: record.geometry,
                        dock: record.dock,
                    });
                }
            }
            Ok(FloatWindowFile::UnsupportedSchema) => {
                self.store
                    .log_warn("todo-float.json 的 schemaVersion 不受支持，叠页还原跳过待办悬浮窗");
            }
            Err(err) => {
                self.store
                    .log_warn(&format!("待办悬浮窗位置没有读出来，叠页还原跳过它：{err}"));
            }
        }
        Some(pages)
    }

    fn ensure_loaded(&mut self, page: &StackPage) {
        if self.member.contains_key(page) {
            return;
        }
        let Some(slot) = self.read_slot(page) else {
            return;
        };
        self.clear_trashed_slots(&slot.id);
        let members = self.members_with_id(&slot.id);
        let mut stacks = assemble_stacks(&members);
        if let Some(index) = stacks.iter().position(|stack| stack.pages.contains(page)) {
            let stack = stacks.swap_remove(index);
            self.store_stack(&stack);
        } else {
            self.write_slot(page, None);
        }
    }

    fn members_with_id(&self, id: &str) -> Vec<StackMember> {
        let mut members = Vec::new();
        if let Some(notes) = &self.notes {
            for note in notes.list() {
                if let Some(slot) = note.float_stack
                    && slot.id == id
                {
                    members.push(StackMember {
                        page: StackPage::Note(note.id),
                        slot,
                    });
                }
            }
        }
        if let Some(shelves) = &self.shelves
            && let Ok(groups) = shelves.list()
        {
            for group in groups {
                if let Some(slot) = group.float_stack
                    && slot.id == id
                {
                    members.push(StackMember {
                        page: StackPage::Shelf(group.id),
                        slot,
                    });
                }
            }
        }
        if let Ok(FloatWindowFile::Record(record)) = self.todos.read_float_window()
            && let Some(slot) = record.stack
            && slot.id == id
        {
            members.push(StackMember {
                page: StackPage::Todo,
                slot,
            });
        }
        members
    }

    fn read_slot(&self, page: &StackPage) -> Option<FloatStackSlot> {
        match page {
            StackPage::Note(id) => self
                .notes
                .as_ref()
                .and_then(|notes| notes.get(id).ok())
                .and_then(|note| note.float_stack),
            StackPage::Shelf(id) => self
                .shelves
                .as_ref()
                .and_then(|shelves| shelves.get(id).ok())
                .and_then(|group| group.float_stack),
            StackPage::Todo => self.todo_record().and_then(|record| record.stack),
        }
    }

    fn disk_saved(&self, page: &StackPage) -> Option<SavedFloat> {
        match page {
            StackPage::Note(id) => {
                let note = self.notes.as_ref()?.get(id).ok()?;
                Some(SavedFloat {
                    geometry: note.float?,
                    dock: note.float_dock,
                })
            }
            StackPage::Shelf(id) => {
                let group = self.shelves.as_ref()?.get(id).ok()?;
                Some(SavedFloat {
                    geometry: group.float?,
                    dock: group.float_dock,
                })
            }
            StackPage::Todo => {
                let record = self.todo_record()?;
                Some(SavedFloat {
                    geometry: record.geometry?,
                    dock: record.dock,
                })
            }
        }
    }

    fn todo_record(&self) -> Option<lanwork_core::todos::FloatWindowRecord> {
        match self.todos.read_float_window() {
            Ok(FloatWindowFile::Record(record)) => Some(record),
            Ok(FloatWindowFile::UnsupportedSchema) => None,
            Err(err) => {
                self.store
                    .log_warn(&format!("待办悬浮窗位置没有读出来：{err}"));
                None
            }
        }
    }

    fn saved_of_shown(&self, stack: &PageStack) -> Option<SavedFloat> {
        stack
            .pages
            .iter()
            .find_map(|page| self.shown.get(page).map(|tracked| tracked.saved))
    }

    fn take_stack(&mut self, page: &StackPage) -> Option<PageStack> {
        let id = self.member.get(page)?.clone();
        let stack = self.stacks.remove(&id)?;
        for item in &stack.pages {
            self.member.remove(item);
        }
        Some(stack)
    }

    fn store_stack(&mut self, stack: &PageStack) {
        for page in &stack.pages {
            self.member.insert(page.clone(), stack.id.clone());
        }
        self.stacks.insert(stack.id.clone(), stack.clone());
    }

    fn persist(&mut self, stack: &PageStack, saved: SavedFloat) {
        for (index, page) in stack.pages.iter().enumerate() {
            let slot = FloatStackSlot {
                id: stack.id.clone(),
                index: u32::try_from(index).unwrap_or(u32::MAX),
                current: index == stack.current,
            };
            self.write_float(page, saved);
            self.write_slot(page, Some(slot));
        }
    }

    fn write_solo(&mut self, page: &StackPage, saved: SavedFloat) {
        self.member.remove(page);
        self.write_float(page, saved);
        self.write_slot(page, None);
    }

    fn write_float(&self, page: &StackPage, saved: SavedFloat) {
        let result = match page {
            StackPage::Note(id) => self.notes.as_ref().and_then(|notes| {
                notes
                    .set_float(id, saved.geometry, saved.dock)
                    .map(|_| ())
                    .map_err(|err| err.to_string())
                    .err()
            }),
            StackPage::Shelf(id) => self.shelves.as_ref().and_then(|shelves| {
                shelves
                    .set_float(id, saved.geometry, saved.dock)
                    .map(|_| ())
                    .map_err(|err| err.to_string())
                    .err()
            }),
            StackPage::Todo => self
                .todos
                .set_float_geometry(saved.geometry, saved.dock)
                .map(|_| ())
                .map_err(|err| err.to_string())
                .err(),
        };
        if let Some(message) = result {
            self.store
                .log_warn(&format!("叠页位置没有写进去：{message}"));
        }
    }

    fn write_slot(&self, page: &StackPage, slot: Option<FloatStackSlot>) {
        let result = match page {
            StackPage::Note(id) => self.notes.as_ref().and_then(|notes| {
                notes
                    .set_stack(id, slot)
                    .map(|_| ())
                    .map_err(|err| err.to_string())
                    .err()
            }),
            StackPage::Shelf(id) => self.shelves.as_ref().and_then(|shelves| {
                shelves
                    .set_stack(id, slot)
                    .map(|_| ())
                    .map_err(|err| err.to_string())
                    .err()
            }),
            StackPage::Todo => self
                .todos
                .set_float_stack(slot)
                .map(|_| ())
                .map_err(|err| err.to_string())
                .err(),
        };
        if let Some(message) = result {
            self.store
                .log_warn(&format!("叠页成员没有写进去：{message}"));
        }
    }

    fn page_openable(&self, page: &StackPage) -> bool {
        match page {
            StackPage::Note(id) => self
                .notes
                .as_ref()
                .and_then(|notes| notes.get(id).ok())
                .is_some_and(|note| !note.is_deleted()),
            StackPage::Shelf(id) => self
                .shelves
                .as_ref()
                .and_then(|shelves| shelves.get(id).ok())
                .is_some(),
            StackPage::Todo => true,
        }
    }

    fn clear_trashed_slots(&self, id: &str) {
        let Some(notes) = &self.notes else {
            return;
        };
        let trashed: Vec<String> = notes
            .deleted()
            .into_iter()
            .filter(|note| note.float_stack.as_ref().is_some_and(|slot| slot.id == id))
            .map(|note| note.id)
            .collect();
        for note_id in trashed {
            self.write_slot(&StackPage::Note(note_id), None);
        }
    }

    fn retain_open(&mut self, stack: &mut PageStack) {
        let filtered = filter_open_pages(stack, |page| self.page_openable(page));
        for page in &filtered.dropped {
            self.write_slot(page, None);
            self.member.remove(page);
        }
        if filtered.dropped.is_empty() {
            return;
        }
        stack.pages = filtered.pages;
        if let Some(current) = filtered.current {
            stack.current = current;
        } else if !stack.pages.is_empty() {
            stack.current = stack
                .pages
                .iter()
                .position(|page| self.shown.contains_key(page))
                .unwrap_or(0);
        }
    }

    /// 不足两页就拆成单独一页。不藏还看得见的那一页。返回真表示已经拆掉。
    fn finish_thin(&mut self, stack: &PageStack, saved: Option<SavedFloat>) -> bool {
        if stack.pages.len() >= 2 {
            return false;
        }
        let Some(left) = stack.pages.first() else {
            return true;
        };
        if let Some(saved) = saved
            .or_else(|| self.disk_saved(left))
            .or_else(|| self.read_view(left).map(|(saved, _)| saved))
        {
            self.write_solo(left, saved);
        } else {
            self.member.remove(left);
            self.write_slot(left, None);
        }
        self.paint_chrome(left, 1, 0);
        true
    }

    fn paint_current(&self, stack: &PageStack) {
        let Some(page) = stack.pages.get(stack.current) else {
            return;
        };
        let count = i32::try_from(stack.pages.len()).unwrap_or(i32::MAX);
        let index = i32::try_from(stack.current).unwrap_or(0);
        self.paint_chrome(page, count, index);
    }

    /// 目标页没显示出来时，把它从叠里拿掉，当前看得见的页留在屏幕上。
    fn abandon_failed_show(&mut self, visible: &StackPage, target: &StackPage, saved: SavedFloat) {
        let Some(mut stack) = self.take_stack(visible).or_else(|| self.take_stack(target)) else {
            self.write_slot(target, None);
            return;
        };
        self.write_slot(target, None);
        stack.pages.retain(|page| page != target);
        if let Some(index) = stack.pages.iter().position(|page| page == visible) {
            stack.current = index;
        } else if !stack.pages.is_empty() {
            stack.current = 0;
        }
        if self.finish_thin(&stack, Some(saved)) {
            return;
        }
        self.store_stack(&stack);
        self.persist(&stack, saved);
        self.paint_current(&stack);
    }

    fn reveal(&mut self, stack: &PageStack, saved: SavedFloat) -> bool {
        let Some(current) = stack.pages.get(stack.current).cloned() else {
            return false;
        };
        let count = i32::try_from(stack.pages.len()).unwrap_or(i32::MAX);
        let index = i32::try_from(stack.current).unwrap_or(0);
        if !self.show_page(&current, saved, count, index) {
            return false;
        }
        for other in pages_to_park(&stack.pages, &current, true) {
            self.park_page(&other);
        }
        true
    }

    fn show_page(&mut self, page: &StackPage, saved: SavedFloat, pages: i32, index: i32) -> bool {
        let opened = match page {
            StackPage::Note(id) => {
                if !note_float::contains(id)
                    && let Err(message) = note_float::open(id)
                {
                    self.store
                        .log_warn(&format!("叠页里的便签没有打开：{message}"));
                    false
                } else {
                    note_float::present(id, saved, pages, index);
                    true
                }
            }
            StackPage::Shelf(id) => {
                if !shelf_float::contains(id)
                    && let Err(message) = shelf_float::open(id)
                {
                    self.store
                        .log_warn(&format!("叠页里的收纳没有打开：{message}"));
                    false
                } else {
                    shelf_float::present(id, saved, pages, index);
                    true
                }
            }
            StackPage::Todo => {
                if !todo_float::contains()
                    && let Err(message) = todo_float::open()
                {
                    self.store
                        .log_warn(&format!("叠页里的待办没有打开：{message}"));
                    false
                } else {
                    todo_float::present(saved, pages, index);
                    true
                }
            }
        };
        if opened {
            self.track(page);
        }
        opened
    }

    fn park_page(&mut self, page: &StackPage) {
        match page {
            StackPage::Note(id) => note_float::park(id),
            StackPage::Shelf(id) => shelf_float::park(id),
            StackPage::Todo => todo_float::park(),
        }
        self.shown.remove(page);
    }

    fn paint_chrome(&self, page: &StackPage, pages: i32, index: i32) {
        match page {
            StackPage::Note(id) => note_float::set_chrome(id, pages, index),
            StackPage::Shelf(id) => shelf_float::set_chrome(id, pages, index),
            StackPage::Todo => todo_float::set_chrome(pages, index),
        }
    }

    fn read_view(&self, page: &StackPage) -> Option<(SavedFloat, PxBox)> {
        match page {
            StackPage::Note(id) => note_float::view(id),
            StackPage::Shelf(id) => shelf_float::view(id),
            StackPage::Todo => todo_float::view(),
        }
    }

    fn track(&mut self, page: &StackPage) {
        if let Some((saved, visible)) = self.read_view(page) {
            self.shown.insert(page.clone(), Tracked { saved, visible });
        }
    }

    fn sync_tabs(&mut self) {
        let mut wanted = Vec::new();
        for (page, tracked) in &self.shown {
            let Some(stack) = self.stack_of(page) else {
                continue;
            };
            if stack.pages.len() < 2 || stack.pages.get(stack.current) != Some(page) {
                continue;
            }
            if tracked.saved.dock.is_some_and(|dock| dock.collapsed) {
                continue;
            }
            let pages = i32::try_from(handle_dot_count(stack.pages.len())).unwrap_or(0);
            let current = i32::try_from(stack.current).unwrap_or(0);
            wanted.push((stack.id.clone(), pages, current, tracked.visible));
        }
        let stale: Vec<String> = self
            .tabs
            .keys()
            .filter(|id| !wanted.iter().any(|(wanted_id, _, _, _)| wanted_id == *id))
            .cloned()
            .collect();
        for id in stale {
            if let Some(tab) = self.tabs.remove(&id) {
                let _ = tab.ui.hide();
            }
        }
        for (id, pages, current, frame) in wanted {
            if !self.tabs.contains_key(&id) && !self.create_tab(&id) {
                continue;
            }
            self.place_tab_ui(&id, pages, current, frame);
        }
    }

    fn create_tab(&mut self, stack_id: &str) -> bool {
        let tab = match StackTab::new() {
            Ok(tab) => tab,
            Err(err) => {
                self.store.log_warn(&format!("叠页页签没有创建：{err}"));
                return false;
            }
        };
        tab.set_dark(self.dark);
        let ui = tab.clone_strong();
        let id = stack_id.to_owned();
        ui.on_drag_dot(move |index| {
            let tab = LIVE.with(|cell| {
                cell.borrow_mut()
                    .as_mut()
                    .and_then(|live| live.begin_tab_drag(&id, index))
            });
            if let Some(tab) = tab {
                let _ = tab.window().with_winit_window(|window| {
                    let _ = window.drag_window();
                });
            }
            // 拖动循环没起来时（点一下就松开），松手消息不会来。下一轮再看要不要换页。
            let click_id = id.clone();
            later(move || {
                with_live(|live| live.click_if_still_pending(&click_id));
            });
        });
        self.tabs.insert(
            stack_id.to_owned(),
            PageTab {
                ui: tab,
                key: None,
                drag: None,
            },
        );
        self.ready_tab(stack_id);
        true
    }

    fn place_tab_ui(&self, stack_id: &str, pages: i32, current: i32, frame: PxBox) {
        let Some(tab) = self.tabs.get(stack_id) else {
            return;
        };
        tab.ui.set_pages(pages.max(0));
        tab.ui.set_current(current.max(0));
        tab.ui.set_dark(self.dark);
        let screens = float_dock::screens();
        let dpi = dpi_at(&screens, frame.x, frame.y);
        let count = usize::try_from(pages.max(0)).unwrap_or(0);
        let (width, height) = dot_row_size(count.max(2), dpi);
        let work = crate::bar_win::monitors().into_iter().find_map(|reading| {
            let on_screen = frame.x >= reading.bounds.left
                && frame.x < reading.bounds.right
                && frame.y >= reading.bounds.top
                && frame.y < reading.bounds.bottom;
            on_screen.then_some(reading.work)
        });
        let placed = fit_dot_row(frame, width, height, work);
        let _ = tab.ui.show();
        float_dock::place(tab.ui.window(), self.tab_hwnd(stack_id), placed);
    }

    fn ready_tab(&mut self, stack_id: &str) {
        let Some(tab) = self.tabs.get(stack_id).map(|tab| tab.ui.clone_strong()) else {
            return;
        };
        let id = stack_id.to_owned();
        let spawned = slint::spawn_local(async move {
            if let Ok(window) = tab.window().winit_window().await {
                use slint::winit_030::winit::platform::windows::WindowExtWindows as _;
                window.set_skip_taskbar(true);
            }
            later(move || {
                with_live(|live| live.finish_tab(&id));
            });
        });
        if let Err(err) = spawned {
            self.store.log_warn(&format!("叠页页签初始化未排队：{err}"));
        }
    }

    fn finish_tab(&mut self, stack_id: &str) {
        let Some(ui) = self.tabs.get(stack_id).map(|tab| tab.ui.clone_strong()) else {
            return;
        };
        let Some(hwnd) = crate::bar_win::hwnd_of(ui.window()) else {
            return;
        };
        if !crate::bar_win::watch_move_loop(hwnd) {
            self.store.log_warn("叠页页签的拖动结束通知没有装上");
        }
        if !crate::bar_win::arm_no_activate(hwnd) {
            self.store.log_warn("叠页页签点下去仍会抢走焦点");
        }
        if !crate::bar_win::watch_topmost(hwnd) {
            self.store.log_warn("叠页页签没有算进前台置顶");
        }
        // 页点不是内容窗，不画来源行。清掉 winit 默认的系统背景，失焦后也不铺不透明的底。
        crate::bar_win::apply_clear_plate(hwnd);
        if let Some(tab) = self.tabs.get_mut(stack_id) {
            tab.key = Some(crate::bar_win::hwnd_key(hwnd));
        }
    }

    fn tab_hwnd(&self, stack_id: &str) -> Option<windows::Win32::Foundation::HWND> {
        self.tabs
            .get(stack_id)
            .and_then(|tab| crate::bar_win::hwnd_of(tab.ui.window()))
    }

    /// 记下按下的页和当时的位置。拖动本身在借用结束之后进行，避免松手回调再借这一层。
    fn begin_tab_drag(&mut self, stack_id: &str, index: i32) -> Option<StackTab> {
        let tab = self.tabs.get(stack_id)?.ui.clone_strong();
        let page_index = usize::try_from(index).ok()?;
        let stack = self.stacks.get(stack_id)?;
        if page_index >= stack.pages.len() || page_index >= handle_dot_count(stack.pages.len()) {
            return None;
        }
        let shown = self.shown_current(stack_id)?;
        let page_at = self.shown.get(&shown).map(|tracked| tracked.visible)?;
        let origin = self
            .tab_hwnd(stack_id)
            .and_then(crate::bar_win::window_origin)
            .unwrap_or((0, 0));
        if let Some(slot) = self.tabs.get_mut(stack_id) {
            slot.drag = Some(TabDrag {
                page_at,
                tab_at: origin,
                page_index,
            });
        }
        Some(tab)
    }

    fn shown_current(&self, stack_id: &str) -> Option<StackPage> {
        let stack = self.stacks.get(stack_id)?;
        let page = stack.pages.get(stack.current)?.clone();
        self.shown.contains_key(&page).then_some(page)
    }

    fn tab_id_for_key(&self, key: isize) -> Option<String> {
        self.tabs
            .iter()
            .find_map(|(id, tab)| (tab.key == Some(key)).then(|| id.clone()))
    }

    fn tab_blocks_blur(&self, page: &StackPage) -> bool {
        let Some(id) = self.member.get(page) else {
            return false;
        };
        let Some(key) = self.tabs.get(id).and_then(|tab| tab.key) else {
            return false;
        };
        crate::bar_win::foreground_is(key) || crate::bar_win::size_move_blocks_blur(key)
    }

    fn tab_released(&mut self, stack_id: &str, x: i32, y: i32) {
        let Some(drag) = self.tabs.get_mut(stack_id).and_then(|tab| tab.drag.take()) else {
            self.sync_tabs();
            return;
        };
        let Some(page) = self
            .stacks
            .get(stack_id)
            .and_then(|stack| stack.pages.get(drag.page_index))
            .cloned()
        else {
            self.sync_tabs();
            return;
        };
        let released = shift(
            drag.page_at,
            x.saturating_sub(drag.tab_at.0),
            y.saturating_sub(drag.tab_at.1),
        );
        let screens = float_dock::screens();
        let dpi = dpi_at(&screens, x, y);
        let moved = moved_enough(drag.tab_at, (x, y), dpi);
        let effect = self.classify(&page, DragOrigin::PageDot, drag.page_at, released, false);
        match effect {
            DropEffect::DetachCurrent => {
                self.detach_dragged(&page, released);
                self.sync_tabs();
            }
            DropEffect::MergeCurrent { target } => {
                let before = self.stack_saved(stack_id);
                if let Some(before) = before {
                    self.split_off(&page, before);
                }
                self.merge_unit(page, target);
            }
            DropEffect::MoveTogether | DropEffect::MergeUnit { .. } => {
                if !moved {
                    self.activate_index(page, drag.page_index);
                    return;
                }
                self.sync_tabs();
            }
        }
    }

    fn click_if_still_pending(&mut self, stack_id: &str) {
        let pending = self
            .tabs
            .get(stack_id)
            .and_then(|tab| tab.drag.as_ref().map(|drag| drag.page_index));
        let Some(page_index) = pending else {
            return;
        };
        let key = self.tabs.get(stack_id).and_then(|tab| tab.key);
        if key.is_some_and(crate::bar_win::size_move_blocks_blur) {
            return;
        }
        if let Some(tab) = self.tabs.get_mut(stack_id) {
            tab.drag = None;
        }
        let Some(page) = self
            .stacks
            .get(stack_id)
            .and_then(|stack| stack.pages.get(page_index))
            .cloned()
        else {
            self.sync_tabs();
            return;
        };
        self.activate_index(page, page_index);
    }

    fn stack_saved(&self, stack_id: &str) -> Option<SavedFloat> {
        let shown = self.shown_current(stack_id)?;
        self.shown
            .get(&shown)
            .map(|tracked| tracked.saved)
            .or_else(|| self.disk_saved(&shown))
    }

    fn detach_dragged(&mut self, page: &StackPage, released: PxBox) {
        let before = self
            .member
            .get(page)
            .and_then(|id| self.stack_saved(id))
            .or_else(|| self.disk_saved(page));
        let Some(before) = before else {
            return;
        };
        self.split_off(page, before);
        let parked = solo_at(before, released);
        self.write_solo(page, parked);
        let _ = self.show_page(page, parked, 1, 0);
    }
}

fn solo_stack(page: &StackPage) -> PageStack {
    PageStack {
        id: new_stack_id(),
        pages: vec![page.clone()],
        current: 0,
    }
}

fn solo_at(before: SavedFloat, released: PxBox) -> SavedFloat {
    let expanded = PxBox {
        x: released.x,
        y: released.y,
        width: before.geometry.width,
        height: before.geometry.height,
    };
    let change = float_dock::release_drag(
        &DockState {
            expanded,
            dock: None,
        },
        expanded,
        true,
        false,
        false,
    );
    saved_from(change.state.expanded, change.state.dock)
}

fn on_tab_move(key: isize, entering: bool, x: i32, y: i32, _width: u32, _height: u32) {
    let posted = slint::invoke_from_event_loop(move || {
        deliver_tab(key, entering, x, y);
    });
    if posted.is_err() {
        deliver_tab(key, entering, x, y);
    }
}

fn deliver_tab(key: isize, entering: bool, x: i32, y: i32) {
    with_live(|live| {
        let Some(id) = live.tab_id_for_key(key) else {
            return;
        };
        if entering {
            let page_index = live
                .tabs
                .get(&id)
                .and_then(|tab| tab.drag.as_ref().map(|drag| drag.page_index))
                .or_else(|| live.stacks.get(&id).map(|stack| stack.current));
            let page = live.shown_current(&id);
            let drag = page.and_then(|page| {
                live.shown.get(&page).map(|tracked| TabDrag {
                    page_at: tracked.visible,
                    tab_at: (x, y),
                    page_index: page_index.unwrap_or(0),
                })
            });
            if let Some(drag) = drag
                && let Some(tab) = live.tabs.get_mut(&id)
            {
                tab.drag = Some(drag);
            }
            return;
        }
        live.tab_released(&id, x, y);
    });
}

/// 前台是这一叠的页签，或页签正在拖动时，内容窗的失焦不收起。
pub(crate) fn companion_blocks_blur(page: &StackPage) -> bool {
    LIVE.with(|cell| {
        let Ok(slot) = cell.try_borrow() else {
            return true;
        };
        slot.as_ref().is_some_and(|live| live.tab_blocks_blur(page))
    })
}
