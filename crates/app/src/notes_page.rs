//! 面板的便签页：列表、标签筛选、回收站，以及选中便签的编辑区。
//!
//! 窗口在 `panel`，版式在 `ui/notespage.slint`。保存流程在 `note_editor`，规则在 `lanwork_core::panel`
//! 和便签服务。这里把服务的结果排成界面要的行，并把用户的动作交给服务。

use std::rc::Rc;

use lanwork_core::notes::{Note, NoteCommands, NoteInput};
use lanwork_core::panel::{NoteRow, NoteView, list_rows, locate_note, tag_names, trash_rows};
use lanwork_core::storage::Store;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::host::unix_now_ms;
use crate::note_editor::Editor;
use crate::panel::with_notes;
use crate::{NoteListRow, NotesPageData, Panel, TagFilter};

/// 保存没有成功时暂存的后续动作。
pub(crate) enum After {
    Select(String),
    Create,
    ShowTrash,
    Delete,
    Quit,
}

pub(crate) struct NotesPage {
    notes: Option<NoteCommands>,
    store: Store,
    rows_model: Rc<VecModel<NoteListRow>>,
    filters_model: Rc<VecModel<TagFilter>>,
    tags_model: Rc<VecModel<SharedString>>,
    view: NoteView,
    tag_filter: Option<String>,
    list: Vec<Note>,
    deleted: Vec<Note>,
    rows: Vec<NoteRow>,
    selected: Option<String>,
    editor: Option<Editor<After>>,
    focus_ticket: i32,
}

fn tick() -> impl Fn() + 'static {
    || with_notes(|page, ui| page.tick(ui))
}

fn input_of(note: &Note) -> NoteInput {
    NoteInput {
        title: note.title.clone(),
        body: note.body.clone(),
        tags: note.tags.clone(),
        pinned: note.pinned,
    }
}

impl NotesPage {
    pub(crate) fn new(ui: &Panel, notes: Option<NoteCommands>, store: Store) -> Self {
        let page = Self {
            notes,
            store,
            rows_model: Rc::default(),
            filters_model: Rc::default(),
            tags_model: Rc::default(),
            view: NoteView::List,
            tag_filter: None,
            list: Vec::new(),
            deleted: Vec::new(),
            rows: Vec::new(),
            selected: None,
            editor: None,
            focus_ticket: 0,
        };
        let data = ui.global::<NotesPageData>();
        data.set_rows(ModelRc::from(Rc::clone(&page.rows_model)));
        data.set_filters(ModelRc::from(Rc::clone(&page.filters_model)));
        data.set_tags(ModelRc::from(Rc::clone(&page.tags_model)));
        data.set_selected(-1);
        wire(ui);
        page
    }

    /// 切到便签标签，或面板在便签标签上显示时调用。读最新列表；编辑区没有未保存的修改时也换成最新内容。
    pub(crate) fn entered(&mut self, ui: &Panel) {
        ui.set_error(SharedString::new());
        self.reload(ui);
        self.refresh_editor(ui);
    }

    /// 其他地方保存了便签之后调用。只重排列表，不动编辑区里正在改的文字。
    pub(crate) fn reload(&mut self, ui: &Panel) {
        let Some(notes) = self.notes.clone() else {
            ui.set_error("便签服务没有打开".into());
            return;
        };
        self.list = notes.list();
        self.deleted = notes.deleted();
        let tags = tag_names(&self.list);
        if self
            .tag_filter
            .as_ref()
            .is_some_and(|tag| !tags.contains(tag))
        {
            self.tag_filter = None;
        }
        self.rows = match self.view {
            NoteView::List => list_rows(&self.list, self.tag_filter.as_deref()),
            NoteView::Trash => trash_rows(&self.deleted, unix_now_ms()),
        };
        self.rows_model.set_vec(
            self.rows
                .iter()
                .map(|row| NoteListRow {
                    title: row.title.as_str().into(),
                    pinned: row.pinned,
                    tags: row.tags.as_str().into(),
                    note: row.trash_note.as_str().into(),
                })
                .collect::<Vec<_>>(),
        );
        self.filters_model.set_vec(
            tags.iter()
                .map(|name| TagFilter {
                    name: name.as_str().into(),
                    active: self.tag_filter.as_deref() == Some(name.as_str()),
                })
                .collect::<Vec<_>>(),
        );
        let data = ui.global::<NotesPageData>();
        data.set_trash(self.view == NoteView::Trash);
        data.set_list_count(count(self.list.len()));
        data.set_trash_count(count(self.deleted.len()));
        data.set_selected(self.selected_index());
    }

    fn selected_index(&self) -> i32 {
        self.selected
            .as_ref()
            .and_then(|id| self.rows.iter().position(|row| &row.id == id))
            .map_or(-1, count)
    }

    fn refresh_editor(&mut self, ui: &Panel) {
        let Some(notes) = self.notes.clone() else {
            return;
        };
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let Ok(latest) = notes.get(editor.session.id()) else {
            return;
        };
        if editor.session.adopt_if_clean(&latest) {
            self.show_note(ui, &latest);
        }
    }

    fn show_note(&self, ui: &Panel, note: &Note) {
        self.show_input(ui, &input_of(note));
    }

    /// 把一份内容放进编辑区，输入框回到这份内容。
    fn show_input(&self, ui: &Panel, input: &NoteInput) {
        let data = ui.global::<NotesPageData>();
        data.set_has_note(true);
        data.set_title(input.title.as_str().into());
        data.set_body(input.body.as_str().into());
        data.set_pinned(input.pinned);
        self.tags_model.set_vec(
            input
                .tags
                .iter()
                .map(|tag| SharedString::from(tag.as_str()))
                .collect::<Vec<_>>(),
        );
        data.set_version(data.get_version().wrapping_add(1));
    }

    fn clear_page(&mut self, ui: &Panel) {
        self.selected = None;
        self.editor = None;
        let data = ui.global::<NotesPageData>();
        data.set_has_note(false);
        data.set_problem(0);
        data.set_closing(false);
        data.set_selected(-1);
    }

    fn push_problem(&self, ui: &Panel) {
        let data = ui.global::<NotesPageData>();
        match &self.editor {
            Some(editor) => {
                let view = editor.problem_view();
                data.set_problem(view.kind);
                data.set_problem_text(view.text);
                data.set_closing(view.closing);
            }
            None => {
                data.set_problem(0);
                data.set_closing(false);
            }
        }
    }

    fn fail(&self, ui: &Panel, message: &str) {
        self.store.log_warn(&format!("便签页：{message}"));
        ui.set_error(message.into());
    }

    /// 托盘「新建便签」。当前编辑区保存不成功时先停在原来的便签上，等用户重试或放弃。
    pub(crate) fn create_new(&mut self, ui: &Panel) {
        self.request(ui, After::Create);
    }

    /// 打开某一篇并选中。保存不成功时先停在原来的便签上，等用户重试或放弃。
    pub(crate) fn locate(&mut self, ui: &Panel, id: &str) {
        self.reload(ui);
        if locate_note(&self.list, &self.deleted, id).is_none() {
            self.fail(ui, "找不到便签");
            return;
        }
        self.request(ui, After::Select(id.to_owned()));
    }

    fn request(&mut self, ui: &Panel, after: After) {
        let Some(notes) = self.notes.clone() else {
            return;
        };
        if let Some(editor) = self.editor.as_mut() {
            if !editor.flush(&notes) {
                editor.after = Some(after);
                self.push_problem(ui);
                return;
            }
            editor.after = None;
        }
        self.perform(ui, after);
    }

    fn perform(&mut self, ui: &Panel, after: After) {
        match after {
            After::Select(id) => self.load(ui, &id),
            After::Create => self.create(ui),
            After::ShowTrash => {
                self.view = NoteView::Trash;
                self.clear_page(ui);
                self.reload(ui);
            }
            After::Delete => self.delete(ui),
            After::Quit => crate::host::continue_quit(),
        }
    }

    fn proceed(&mut self, ui: &Panel) {
        let after = self.editor.as_mut().and_then(|editor| editor.after.take());
        self.push_problem(ui);
        if let Some(after) = after {
            self.perform(ui, after);
        }
    }

    fn load(&mut self, ui: &Panel, id: &str) {
        let Some(notes) = self.notes.clone() else {
            return;
        };
        let note = match notes.get(id) {
            Ok(note) => note,
            Err(err) => {
                self.fail(ui, &err.to_string());
                return;
            }
        };
        ui.set_error(SharedString::new());
        self.view = if note.is_deleted() {
            NoteView::Trash
        } else {
            NoteView::List
        };
        self.selected = Some(note.id.clone());
        self.editor = (self.view == NoteView::List).then(|| Editor::new(&note));
        self.show_note(ui, &note);
        self.push_problem(ui);
        self.reload(ui);
    }

    fn create(&mut self, ui: &Panel) {
        let Some(notes) = self.notes.clone() else {
            return;
        };
        let blank = NoteInput {
            title: String::new(),
            body: String::new(),
            tags: Vec::new(),
            pinned: false,
        };
        match notes.create(&blank) {
            Ok(note) => {
                self.tag_filter = None;
                self.load(ui, &note.id);
                self.focus_body(ui);
            }
            Err(err) => self.fail(ui, &err.to_string()),
        }
    }

    /// 编辑区建好之后再给焦点，所以放到稍后的一轮。
    fn focus_body(&mut self, ui: &Panel) {
        self.focus_ticket = self.focus_ticket.wrapping_add(1);
        let ticket = self.focus_ticket;
        let weak = ui.as_weak();
        slint::Timer::single_shot(std::time::Duration::from_millis(60), move || {
            if let Some(ui) = weak.upgrade() {
                ui.global::<NotesPageData>().set_focus_ticket(ticket);
            }
        });
    }

    fn delete(&mut self, ui: &Panel) {
        let (Some(notes), Some(editor)) = (self.notes.clone(), self.editor.as_ref()) else {
            return;
        };
        let (id, revision) = (editor.session.id().to_owned(), editor.session.revision());
        match notes.soft_delete(&id, revision) {
            Ok(_) => {
                ui.set_error(SharedString::new());
                self.clear_page(ui);
                self.reload(ui);
            }
            Err(err) => self.fail(ui, &err.to_string()),
        }
    }

    /// 回收站里恢复或永久删除选中的一篇。
    fn trash_action(&mut self, ui: &Panel, purge: bool) {
        let (Some(notes), Some(id)) = (self.notes.clone(), self.selected.clone()) else {
            return;
        };
        let result = notes.get(&id).and_then(|note| {
            if purge {
                notes.purge(&id, note.revision)
            } else {
                notes.restore(&id, note.revision).map(|_| ())
            }
        });
        match result {
            Ok(()) => {
                ui.set_error(SharedString::new());
                self.clear_page(ui);
                self.reload(ui);
            }
            Err(err) => self.fail(ui, &err.to_string()),
        }
    }

    fn edit(&mut self, change: impl FnOnce(&mut lanwork_core::panel::EditSession, i64)) {
        if let Some(editor) = self.editor.as_mut() {
            editor.edit(change, tick());
        }
    }

    fn save_now(&mut self, ui: &Panel) {
        if let (Some(notes), Some(editor)) = (self.notes.clone(), self.editor.as_mut()) {
            editor.flush(&notes);
        }
        self.push_problem(ui);
    }

    fn tick(&mut self, ui: &Panel) {
        let (Some(notes), Some(editor)) = (self.notes.clone(), self.editor.as_mut()) else {
            return;
        };
        let preedit_empty = ui.global::<NotesPageData>().get_preedit().is_empty();
        editor.on_tick(preedit_empty, &notes, tick());
        self.push_problem(ui);
    }

    fn push_tags(&self) {
        if let Some(editor) = &self.editor {
            self.tags_model.set_vec(
                editor
                    .session
                    .draft()
                    .tags
                    .iter()
                    .map(|tag| SharedString::from(tag.as_str()))
                    .collect::<Vec<_>>(),
            );
        }
    }

    fn add_tag(&mut self, ui: &Panel, text: &str) {
        let now = unix_now_ms();
        let changed = self
            .editor
            .as_mut()
            .is_some_and(|editor| editor.session.add_tag(text, now));
        let data = ui.global::<NotesPageData>();
        data.set_tag_reset(data.get_tag_reset().wrapping_add(1));
        if changed {
            self.push_tags();
            self.save_now(ui);
        }
    }

    fn remove_tag(&mut self, ui: &Panel, tag: &str) {
        let now = unix_now_ms();
        let changed = self
            .editor
            .as_mut()
            .is_some_and(|editor| editor.session.remove_tag(tag, now));
        if changed {
            self.push_tags();
            self.save_now(ui);
        }
    }

    fn toggle_pin(&mut self, ui: &Panel) {
        let now = unix_now_ms();
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let pinned = !editor.session.draft().pinned;
        editor.session.set_pinned(pinned, now);
        ui.global::<NotesPageData>().set_pinned(pinned);
        self.save_now(ui);
    }

    fn float_note(&mut self, ui: &Panel) {
        let (Some(notes), Some(editor)) = (self.notes.clone(), self.editor.as_mut()) else {
            return;
        };
        editor.flush(&notes);
        let id = editor.session.id().to_owned();
        self.push_problem(ui);
        if let Err(message) = crate::note_float::open(&id) {
            self.fail(ui, &message);
        }
    }

    fn retry(&mut self, ui: &Panel) {
        let (Some(notes), Some(editor)) = (self.notes.clone(), self.editor.as_mut()) else {
            return;
        };
        if editor.flush(&notes) {
            self.proceed(ui);
        } else {
            self.push_problem(ui);
        }
    }

    fn discard(&mut self, ui: &Panel) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        editor.session.discard();
        let input = editor.session.draft().clone();
        self.show_input(ui, &input);
        self.proceed(ui);
    }

    fn keep_mine(&mut self, ui: &Panel) {
        let (Some(notes), Some(editor)) = (self.notes.clone(), self.editor.as_mut()) else {
            return;
        };
        editor.session.keep_mine(&notes);
        if editor.resolved() {
            self.proceed(ui);
        } else {
            self.push_problem(ui);
        }
    }

    fn keep_saved(&mut self, ui: &Panel) {
        let (Some(notes), Some(editor)) = (self.notes.clone(), self.editor.as_mut()) else {
            return;
        };
        match editor.session.keep_saved(&notes) {
            Some(latest) => {
                self.show_note(ui, &latest);
                self.proceed(ui);
            }
            None => self.push_problem(ui),
        }
    }

    /// 退出前保存。返回保存之后仍没有写盘的窗口数（0 或 1）。没写成时记下退出这个后续动作。
    pub(crate) fn flush_for_quit(&mut self, ui: &Panel) -> usize {
        let Some(notes) = self.notes.clone() else {
            return 0;
        };
        let Some(editor) = self.editor.as_mut() else {
            return 0;
        };
        if editor.flush(&notes) {
            return 0;
        }
        editor.after = Some(After::Quit);
        self.push_problem(ui);
        1
    }
}

fn count(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn wire(ui: &Panel) {
    let data = ui.global::<NotesPageData>();
    data.on_show_list(|| {
        with_notes(|page, ui| {
            if page.view == NoteView::Trash {
                page.view = NoteView::List;
                page.clear_page(ui);
                page.reload(ui);
            }
        });
    });
    data.on_show_trash(|| {
        with_notes(|page, ui| {
            if page.view != NoteView::Trash {
                page.request(ui, After::ShowTrash);
            }
        });
    });
    data.on_new_note(|| with_notes(|page, ui| page.request(ui, After::Create)));
    data.on_select_note(|index| {
        with_notes(move |page, ui| {
            let id = usize::try_from(index)
                .ok()
                .and_then(|index| page.rows.get(index))
                .map(|row| row.id.clone());
            if let Some(id) = id {
                page.request(ui, After::Select(id));
            }
        });
    });
    data.on_toggle_filter(|name| {
        let name = name.to_string();
        with_notes(move |page, ui| {
            page.tag_filter = if page.tag_filter.as_deref() == Some(name.as_str()) {
                None
            } else {
                Some(name)
            };
            page.reload(ui);
        });
    });
    data.on_title_edited(|text| {
        let text = text.to_string();
        with_notes(move |page, _| page.edit(|session, now| session.set_title(&text, now)));
    });
    data.on_body_edited(|text| {
        let text = text.to_string();
        with_notes(move |page, _| page.edit(|session, now| session.set_body(&text, now)));
    });
    data.on_add_tag(|text| {
        let text = text.to_string();
        with_notes(move |page, ui| page.add_tag(ui, &text));
    });
    data.on_remove_tag(|tag| {
        let tag = tag.to_string();
        with_notes(move |page, ui| page.remove_tag(ui, &tag));
    });
    data.on_toggle_pin(|| with_notes(NotesPage::toggle_pin));
    data.on_float_note(|| with_notes(NotesPage::float_note));
    data.on_delete_note(|| with_notes(|page, ui| page.request(ui, After::Delete)));
    data.on_restore_note(|| with_notes(|page, ui| page.trash_action(ui, false)));
    data.on_purge_note(|| with_notes(|page, ui| page.trash_action(ui, true)));
    data.on_retry(|| with_notes(NotesPage::retry));
    data.on_discard(|| with_notes(NotesPage::discard));
    data.on_keep_mine(|| with_notes(NotesPage::keep_mine));
    data.on_keep_saved(|| with_notes(NotesPage::keep_saved));
}
