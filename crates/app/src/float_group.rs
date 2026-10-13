//! 便签和收纳悬浮窗的叠页。几何在 `lanwork_core::panel::float_stack`，这里只接窗口。
//!
//! 一叠同一时刻只显示一页。其余页的窗口藏着，不释放。页签是挂在边框外的小窗。
//! 关掉窗口不拆叠，成员还在文件里；再悬浮时用记下的当前页和贴边。
//! 下面没写进产品规格的做法都待定。

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lanwork_core::notes::{FloatGeometry, FloatStackSlot, NoteCommands};
use lanwork_core::panel::{
    DockInput, DockState, DragOrigin, DropEffect, DropInput, Flip, OtherFloat, PageStack, PxBox,
    StackMember, StackPage, TAB_LOGICAL_HEIGHT, TAB_LOGICAL_WIDTH, assemble_stacks,
    collapsed_handles_close, detach_page, dock_merge_gap, dpi_at, flip_index, merge_stacks,
    moved_enough, page_label, place_tab, snap_on_release, stack_drop_effect, transition,
};
use lanwork_core::shelves::ShelfCommands;
use lanwork_core::storage::Store;
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, SharedString};

use crate::float_dock::{self, SavedFloat};
use crate::{StackTab, note_float, shelf_float};

struct Tracked {
    saved: SavedFloat,
    visible: PxBox,
}

struct TabDrag {
    page_at: PxBox,
    tab_at: (i32, i32),
}

struct Live {
    notes: Option<NoteCommands>,
    shelves: Option<ShelfCommands>,
    store: Store,
    stacks: HashMap<String, PageStack>,
    member: HashMap<StackPage, String>,
    shown: HashMap<StackPage, Tracked>,
    tab: Option<StackTab>,
    tab_key: Option<isize>,
    tab_drag: Option<TabDrag>,
    dark: bool,
    merge_depth: u8,
}

thread_local! {
    static LIVE: RefCell<Option<Live>> = const { RefCell::new(None) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn install(notes: Option<NoteCommands>, shelves: Option<ShelfCommands>, store: Store) {
    crate::bar_win::add_move_hook(on_tab_move);
    LIVE.with(|cell| {
        *cell.borrow_mut() = Some(Live {
            notes,
            shelves,
            store,
            stacks: HashMap::new(),
            member: HashMap::new(),
            shown: HashMap::new(),
            tab: None,
            tab_key: None,
            tab_drag: None,
            dark: false,
            merge_depth: 0,
        });
    });
}

pub(crate) fn set_dark(dark: bool) {
    with_live(move |live| {
        live.dark = dark;
        if let Some(tab) = &live.tab {
            tab.set_dark(dark);
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
        live.sync_tab();
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
            let Some(mut stack) = self.take_stack(&page) else {
                return;
            };
            if let Some(index) = stack.pages.iter().position(|item| item == &page) {
                stack.current = index;
            }
            let saved = self
                .saved_of_shown(&stack)
                .or_else(|| self.disk_saved(&page))
                .or_else(|| self.read_view(&page).map(|(saved, _)| saved));
            self.store_stack(&stack);
            if let Some(saved) = saved {
                self.persist(&stack, saved);
                let count = i32::try_from(stack.pages.len()).unwrap_or(i32::MAX);
                let index = i32::try_from(stack.current).unwrap_or(0);
                let current = stack.pages[stack.current].clone();
                self.show_page(&current, saved, count, index);
                for other in stack.pages.clone() {
                    if other != current {
                        self.park_page(&other);
                    }
                }
            }
            self.sync_tab();
        } else {
            self.paint_chrome(&page, 1, 0);
            self.track(&page);
            self.sync_tab();
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
        self.sync_tab();
        self.consider_dock_merge(&page);
    }

    fn flip(&mut self, page: StackPage, dir: Flip) {
        self.ensure_loaded(&page);
        let Some(mut stack) = self.take_stack(&page) else {
            return;
        };
        let Some(next) = flip_index(&stack, dir) else {
            self.store_stack(&stack);
            return;
        };
        stack.current = next;
        let saved = self
            .read_view(&page)
            .map(|(saved, _)| saved)
            .or_else(|| self.disk_saved(&page));
        self.store_stack(&stack);
        let Some(saved) = saved else {
            return;
        };
        self.persist(&stack, saved);
        let count = i32::try_from(stack.pages.len()).unwrap_or(i32::MAX);
        let index = i32::try_from(next).unwrap_or(0);
        let show = stack.pages[next].clone();
        self.show_page(&show, saved, count, index);
        for other in stack.pages {
            if other != show {
                self.park_page(&other);
            }
        }
        self.sync_tab();
    }

    fn detach_title(&mut self, page: StackPage, before: SavedFloat, detached: SavedFloat) {
        self.split_off(&page, before);
        self.write_solo(&page, detached);
        self.paint_chrome(&page, 1, 0);
        self.track(&page);
        self.sync_tab();
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
        let count = i32::try_from(merged.pages.len()).unwrap_or(i32::MAX);
        let index = i32::try_from(merged.current).unwrap_or(0);
        let current = merged.pages[merged.current].clone();
        self.show_page(&current, stay, count, index);
        for other in merged.pages.clone() {
            if other != current {
                self.park_page(&other);
            }
        }
        self.sync_tab();
        let depth_page = current.clone();
        self.consider_dock_merge(&depth_page);
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
            let count = i32::try_from(rest.pages.len()).unwrap_or(i32::MAX);
            let index = i32::try_from(rest.current).unwrap_or(0);
            let show = rest.pages[rest.current].clone();
            self.show_page(&show, stay, count, index);
            for other in rest.pages {
                if other != show {
                    self.park_page(&other);
                }
            }
        } else if let Some(left) = outcome.leftover {
            self.write_solo(&left, stay);
            self.show_page(&left, stay, 1, 0);
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
            collapsed_handles_close(visible, dock.edge, place.visible, other_dock.edge, gap)
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

    fn ensure_loaded(&mut self, page: &StackPage) {
        if self.member.contains_key(page) {
            return;
        }
        let Some(slot) = self.read_slot(page) else {
            return;
        };
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
            let notes = notes.list().into_iter().chain(notes.deleted());
            for note in notes {
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
        };
        if let Some(message) = result {
            self.store
                .log_warn(&format!("叠页成员没有写进去：{message}"));
        }
    }

    fn show_page(&mut self, page: &StackPage, saved: SavedFloat, pages: i32, index: i32) {
        match page {
            StackPage::Note(id) => {
                if !note_float::contains(id)
                    && let Err(message) = note_float::open(id)
                {
                    self.store
                        .log_warn(&format!("叠页里的便签没有打开：{message}"));
                    return;
                }
                note_float::present(id, saved, pages, index);
            }
            StackPage::Shelf(id) => {
                if !shelf_float::contains(id)
                    && let Err(message) = shelf_float::open(id)
                {
                    self.store
                        .log_warn(&format!("叠页里的收纳没有打开：{message}"));
                    return;
                }
                shelf_float::present(id, saved, pages, index);
            }
        }
        self.track(page);
    }

    fn park_page(&mut self, page: &StackPage) {
        match page {
            StackPage::Note(id) => note_float::park(id),
            StackPage::Shelf(id) => shelf_float::park(id),
        }
        self.shown.remove(page);
    }

    fn paint_chrome(&self, page: &StackPage, pages: i32, index: i32) {
        match page {
            StackPage::Note(id) => note_float::set_chrome(id, pages, index),
            StackPage::Shelf(id) => shelf_float::set_chrome(id, pages, index),
        }
    }

    fn read_view(&self, page: &StackPage) -> Option<(SavedFloat, PxBox)> {
        match page {
            StackPage::Note(id) => note_float::view(id),
            StackPage::Shelf(id) => shelf_float::view(id),
        }
    }

    fn track(&mut self, page: &StackPage) {
        if let Some((saved, visible)) = self.read_view(page) {
            self.shown.insert(page.clone(), Tracked { saved, visible });
        }
    }

    fn sync_tab(&mut self) {
        let current = self.shown.iter().find_map(|(page, tracked)| {
            let stack = self.stack_of(page)?;
            if stack.pages.len() < 2 || stack.pages.get(stack.current) != Some(page) {
                return None;
            }
            if tracked.saved.dock.is_some_and(|dock| dock.collapsed) {
                return None;
            }
            Some((
                page_label(stack.current, stack.pages.len()),
                tracked.visible,
            ))
        });
        let Some((label, frame)) = current else {
            if let Some(tab) = &self.tab {
                let _ = tab.hide();
            }
            return;
        };
        if self.tab.is_none() {
            match StackTab::new() {
                Ok(tab) => {
                    tab.set_dark(self.dark);
                    let ui = tab.clone_strong();
                    ui.on_drag_tab(move || {
                        let tab = LIVE.with(|cell| {
                            cell.borrow_mut()
                                .as_mut()
                                .and_then(|live| live.begin_tab_drag())
                        });
                        if let Some(tab) = tab {
                            let _ = tab.window().with_winit_window(|window| {
                                let _ = window.drag_window();
                            });
                        }
                    });
                    self.tab = Some(tab);
                    self.ready_tab();
                }
                Err(err) => {
                    self.store.log_warn(&format!("叠页页签没有创建：{err}"));
                    return;
                }
            }
        }
        let Some(tab) = &self.tab else {
            return;
        };
        tab.set_label(SharedString::from(label));
        tab.set_dark(self.dark);
        let screens = float_dock::screens();
        let dpi = dpi_at(&screens, frame.x, frame.y);
        let width = u32::try_from(to_px(TAB_LOGICAL_WIDTH, dpi)).unwrap_or(TAB_LOGICAL_WIDTH);
        let height = u32::try_from(to_px(TAB_LOGICAL_HEIGHT, dpi)).unwrap_or(TAB_LOGICAL_HEIGHT);
        let screen = screens.iter().find(|screen| {
            frame.x >= screen.left
                && frame.x < screen.right
                && frame.y >= screen.top
                && frame.y < screen.bottom
        });
        let placed = place_tab(
            frame,
            width,
            height,
            screen.map(|screen| screen.top),
            screen.map(|screen| screen.bottom),
        );
        let _ = tab.show();
        float_dock::place(tab.window(), self.tab_hwnd(), placed);
    }

    fn ready_tab(&mut self) {
        let Some(tab) = self.tab.as_ref().map(|tab| tab.clone_strong()) else {
            return;
        };
        let spawned = slint::spawn_local(async move {
            if let Ok(window) = tab.window().winit_window().await {
                use slint::winit_030::winit::platform::windows::WindowExtWindows as _;
                window.set_skip_taskbar(true);
            }
            later(|| {
                with_live(|live| {
                    if let Some(tab) = &live.tab
                        && let Some(hwnd) = crate::bar_win::hwnd_of(tab.window())
                    {
                        if !crate::bar_win::watch_move_loop(hwnd) {
                            live.store.log_warn("叠页页签的拖动结束通知没有装上");
                        }
                        live.tab_key = Some(crate::bar_win::hwnd_key(hwnd));
                        crate::bar_win::place_topmost(
                            hwnd,
                            tab.window().position().x,
                            tab.window().position().y,
                            tab.window().size().width,
                            tab.window().size().height,
                        );
                    }
                });
            });
        });
        if let Err(err) = spawned {
            self.store.log_warn(&format!("叠页页签初始化未排队：{err}"));
        }
    }

    fn tab_hwnd(&self) -> Option<windows::Win32::Foundation::HWND> {
        self.tab
            .as_ref()
            .and_then(|tab| crate::bar_win::hwnd_of(tab.window()))
    }

    /// 记下按下时的位置。拖动本身在借用结束之后进行，避免松手回调再借这一层。
    fn begin_tab_drag(&mut self) -> Option<StackTab> {
        let tab = self.tab.as_ref()?.clone_strong();
        let page_at = self
            .current_shown_page()
            .and_then(|page| self.shown.get(&page).map(|tracked| tracked.visible))?;
        let origin = self
            .tab_hwnd()
            .and_then(crate::bar_win::window_origin)
            .unwrap_or((0, 0));
        self.tab_drag = Some(TabDrag {
            page_at,
            tab_at: origin,
        });
        Some(tab)
    }

    fn current_shown_page(&self) -> Option<StackPage> {
        self.shown.iter().find_map(|(page, _)| {
            let stack = self.stack_of(page)?;
            (stack.pages.get(stack.current) == Some(page)).then(|| page.clone())
        })
    }

    fn tab_released(&mut self, x: i32, y: i32) {
        let Some(drag) = self.tab_drag.take() else {
            self.sync_tab();
            return;
        };
        let Some(page) = self.current_shown_page() else {
            self.sync_tab();
            return;
        };
        let released = shift(
            drag.page_at,
            x.saturating_sub(drag.tab_at.0),
            y.saturating_sub(drag.tab_at.1),
        );
        let effect = self.classify(&page, DragOrigin::PageTab, drag.page_at, released, false);
        match effect {
            DropEffect::DetachCurrent => {
                let before = self
                    .shown
                    .get(&page)
                    .map(|tracked| tracked.saved)
                    .or_else(|| self.disk_saved(&page));
                if let Some(before) = before {
                    self.split_off(&page, before);
                    let parked = solo_at(before, released);
                    self.write_solo(&page, parked);
                    self.show_page(&page, parked, 1, 0);
                }
                self.sync_tab();
            }
            DropEffect::MergeCurrent { target } => {
                let before = self
                    .shown
                    .get(&page)
                    .map(|tracked| tracked.saved)
                    .or_else(|| self.disk_saved(&page));
                if let Some(before) = before {
                    self.split_off(&page, before);
                }
                self.merge_unit(page, target);
            }
            DropEffect::MoveTogether | DropEffect::MergeUnit { .. } => self.sync_tab(),
        }
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
    let screens = float_dock::screens();
    let next = transition(
        &DockState {
            expanded,
            dock: None,
        },
        &screens,
        DockInput::DragFinished {
            rect: expanded,
            moved: true,
            from_collapsed: false,
            size_changed: false,
        },
    );
    saved_from(next.state.expanded, next.state.dock)
}

fn to_px(logical: u32, dpi: u32) -> i32 {
    lanwork_core::shell::to_physical(logical, dpi)
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
        if live.tab_key != Some(key) {
            return;
        }
        if entering {
            if let Some(page) = live.current_shown_page()
                && let Some(tracked) = live.shown.get(&page)
            {
                live.tab_drag = Some(TabDrag {
                    page_at: tracked.visible,
                    tab_at: (x, y),
                });
            }
            return;
        }
        live.tab_released(x, y);
    });
}
