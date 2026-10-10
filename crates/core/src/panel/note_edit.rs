//! 一个便签编辑窗口的保存状态：加载时的 `revision`、未保存的草稿、停止编辑 1 秒后的自动保存，
//! 以及保存失败和冲突之后的选择。面板的便签页和每个悬浮窗各持有一份。
//!
//! 不读时钟，毫秒时间由调用方传入。不调用窗口 API。规则的来源是产品规格「便签」：
//! 停止编辑 1 秒后自动保存；同一篇在两处同时修改时，后保存的一方先出现选择版本的提示，不直接覆盖；
//! 保存失败时正文留在窗口里，可以重试，或放弃修改。

use crate::notes::{Note, NoteCommands, NoteError, NoteInput, normalize_tags};

/// 停止编辑多久之后自动保存。产品规格「便签」写明为 1 秒。
pub const AUTOSAVE_IDLE_MS: i64 = 1000;

/// 上一次保存没有成功的原因。草稿仍在会话里。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveProblem {
    /// 写盘或其他错误。文字可以直接显示。
    Failed(String),
    /// 磁盘上的版本比加载时新。要由用户选择保留哪个版本。
    Conflict,
}

/// 一次保存尝试的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveOutcome {
    /// 草稿与已保存内容相同，没有写盘。
    Clean,
    Saved,
    /// 没有保存。原因在 [`EditSession::problem`]。
    Problem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditSession {
    id: String,
    revision: u64,
    saved: NoteInput,
    draft: NoteInput,
    last_edit_ms: i64,
    problem: Option<SaveProblem>,
}

fn input_of(note: &Note) -> NoteInput {
    NoteInput {
        title: note.title.clone(),
        body: note.body.clone(),
        tags: note.tags.clone(),
        pinned: note.pinned,
    }
}

impl EditSession {
    #[must_use]
    pub fn new(note: &Note) -> Self {
        let input = input_of(note);
        Self {
            id: note.id.clone(),
            revision: note.revision,
            saved: input.clone(),
            draft: input,
            last_edit_ms: 0,
            problem: None,
        }
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn draft(&self) -> &NoteInput {
        &self.draft
    }

    #[must_use]
    pub fn problem(&self) -> Option<&SaveProblem> {
        self.problem.as_ref()
    }

    /// 有没有写在窗口里、还没有成功写盘的修改。
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.draft != self.saved
    }

    pub fn set_title(&mut self, title: &str, now_ms: i64) {
        if self.draft.title != title {
            title.clone_into(&mut self.draft.title);
            self.last_edit_ms = now_ms;
        }
    }

    pub fn set_body(&mut self, body: &str, now_ms: i64) {
        if self.draft.body != body {
            body.clone_into(&mut self.draft.body);
            self.last_edit_ms = now_ms;
        }
    }

    pub fn set_pinned(&mut self, pinned: bool, now_ms: i64) {
        if self.draft.pinned != pinned {
            self.draft.pinned = pinned;
            self.last_edit_ms = now_ms;
        }
    }

    /// 加一个标签。规则与服务相同：去掉首尾空白，空的丢掉，已有的不重复。返回是否有变化。
    pub fn add_tag(&mut self, tag: &str, now_ms: i64) -> bool {
        let mut tags = self.draft.tags.clone();
        tags.push(tag.to_owned());
        let tags = normalize_tags(tags);
        if tags == self.draft.tags {
            return false;
        }
        self.draft.tags = tags;
        self.last_edit_ms = now_ms;
        true
    }

    pub fn remove_tag(&mut self, tag: &str, now_ms: i64) -> bool {
        let before = self.draft.tags.len();
        self.draft.tags.retain(|existing| existing != tag);
        let changed = self.draft.tags.len() != before;
        if changed {
            self.last_edit_ms = now_ms;
        }
        changed
    }

    /// 输入法还在组合、文字没有上屏时，把停止编辑的时间重新算起：组合期间不自动保存。
    pub fn postpone(&mut self, now_ms: i64) {
        if self.is_dirty() {
            self.last_edit_ms = now_ms;
        }
    }

    /// 距离自动保存还要等多少毫秒。没有未保存的修改，或正在等用户解决冲突时没有这一步。
    #[must_use]
    pub fn autosave_wait_ms(&self, now_ms: i64) -> Option<i64> {
        if !self.is_dirty() || self.problem == Some(SaveProblem::Conflict) {
            return None;
        }
        let idle = now_ms.saturating_sub(self.last_edit_ms);
        Some((AUTOSAVE_IDLE_MS - idle).max(0))
    }

    /// 停止编辑已满 1 秒。保存失败之后不会自己重试，下一次编辑把停止时间重新算起，到点再保存。
    #[must_use]
    pub fn autosave_due(&self, now_ms: i64) -> bool {
        self.autosave_wait_ms(now_ms) == Some(0)
    }

    /// 用加载时的 `revision` 保存草稿。成功后记下新的 `revision`；失败时草稿不动。
    pub fn save(&mut self, notes: &NoteCommands) -> SaveOutcome {
        if !self.is_dirty() {
            return SaveOutcome::Clean;
        }
        match notes.save(&self.id, self.revision, &self.draft) {
            Ok(note) => {
                self.revision = note.revision;
                self.saved = self.draft.clone();
                self.problem = None;
                SaveOutcome::Saved
            }
            Err(NoteError::Conflict { .. }) => {
                self.problem = Some(SaveProblem::Conflict);
                SaveOutcome::Problem
            }
            Err(err) => {
                self.problem = Some(SaveProblem::Failed(err.to_string()));
                SaveOutcome::Problem
            }
        }
    }

    /// 冲突之后保留本窗口的版本：采用磁盘上现在的 `revision`，再保存草稿。
    pub fn keep_mine(&mut self, notes: &NoteCommands) -> SaveOutcome {
        match notes.get(&self.id) {
            Ok(latest) => {
                self.revision = latest.revision;
                self.problem = None;
                if self.draft == input_of(&latest) {
                    self.saved = self.draft.clone();
                    return SaveOutcome::Clean;
                }
                self.save(notes)
            }
            Err(err) => {
                self.problem = Some(SaveProblem::Failed(err.to_string()));
                SaveOutcome::Problem
            }
        }
    }

    /// 冲突之后保留磁盘上已经保存的版本，丢掉本窗口的草稿。成功时返回要显示的便签。
    pub fn keep_saved(&mut self, notes: &NoteCommands) -> Option<Note> {
        match notes.get(&self.id) {
            Ok(latest) => {
                self.adopt(&latest);
                Some(latest)
            }
            Err(err) => {
                self.problem = Some(SaveProblem::Failed(err.to_string()));
                None
            }
        }
    }

    /// 放弃本窗口未保存的修改，回到上一次成功保存或加载的内容。
    pub fn discard(&mut self) {
        self.draft = self.saved.clone();
        self.problem = None;
    }

    /// 没有未保存的修改时，换成磁盘上更新的内容。返回窗口里的文字是否要跟着换。
    pub fn adopt_if_clean(&mut self, latest: &Note) -> bool {
        if self.is_dirty() || self.problem.is_some() || latest.revision == self.revision {
            return false;
        }
        self.adopt(latest);
        true
    }

    fn adopt(&mut self, note: &Note) {
        let input = input_of(note);
        self.revision = note.revision;
        self.saved = input.clone();
        self.draft = input;
        self.problem = None;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicI64, Ordering};

    use super::*;
    use crate::notes::TimestampMillis;
    use crate::storage::test_temp::TempDir;
    use crate::storage::{Store, StorePaths};

    fn open(root: &std::path::Path) -> NoteCommands {
        let store = Store::open(StorePaths {
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            user_profile: root.join("profile"),
            local_app_data: root.join("local"),
        })
        .unwrap();
        let clock = Arc::new(AtomicI64::new(1_000));
        NoteCommands::open_at(store, move || {
            TimestampMillis::from_millis(clock.fetch_add(1, Ordering::SeqCst))
        })
        .unwrap()
    }

    fn create(notes: &NoteCommands, body: &str) -> Note {
        notes
            .create(&NoteInput {
                title: "标题".to_owned(),
                body: body.to_owned(),
                tags: Vec::new(),
                pinned: false,
            })
            .unwrap()
    }

    #[test]
    fn autosave_waits_one_second_after_the_last_edit() {
        let temp = TempDir::new();
        let notes = open(temp.path());
        let note = create(&notes, "a");
        let mut session = EditSession::new(&note);
        assert_eq!(session.autosave_wait_ms(5_000), None);
        session.set_body("ab", 5_000);
        assert_eq!(session.autosave_wait_ms(5_000), Some(1_000));
        assert_eq!(session.autosave_wait_ms(5_400), Some(600));
        assert!(!session.autosave_due(5_999));
        assert!(session.autosave_due(6_000));
        session.set_body("abc", 5_900);
        assert!(!session.autosave_due(6_000));
        assert!(session.autosave_due(6_900));
    }

    #[test]
    fn composing_postpones_the_autosave_only_while_dirty() {
        let temp = TempDir::new();
        let notes = open(temp.path());
        let note = create(&notes, "a");
        let mut session = EditSession::new(&note);
        session.postpone(500);
        assert_eq!(session.autosave_wait_ms(500), None);
        session.set_body("ab", 1_000);
        session.postpone(1_900);
        assert!(!session.autosave_due(2_000));
        assert!(session.autosave_due(2_900));
    }

    #[test]
    fn same_text_is_not_an_edit() {
        let temp = TempDir::new();
        let notes = open(temp.path());
        let note = create(&notes, "a");
        let mut session = EditSession::new(&note);
        session.set_body("a", 9_000);
        session.set_title("标题", 9_000);
        session.set_pinned(false, 9_000);
        assert!(!session.is_dirty());
        assert!(!session.add_tag("  ", 9_000));
        assert!(!session.remove_tag("没有", 9_000));
        assert_eq!(session.autosave_wait_ms(9_000), None);
    }

    #[test]
    fn save_writes_once_and_a_second_save_is_clean() {
        let temp = TempDir::new();
        let notes = open(temp.path());
        let note = create(&notes, "a");
        let mut session = EditSession::new(&note);
        session.set_body("新正文", 10);
        assert_eq!(session.save(&notes), SaveOutcome::Saved);
        assert_eq!(session.revision(), note.revision + 1);
        assert!(!session.is_dirty());
        assert_eq!(notes.get(&note.id).unwrap().body, "新正文");
        assert_eq!(session.save(&notes), SaveOutcome::Clean);
        assert_eq!(notes.get(&note.id).unwrap().revision, note.revision + 1);
    }

    #[test]
    fn tags_are_normalized_like_the_service() {
        let temp = TempDir::new();
        let notes = open(temp.path());
        let note = create(&notes, "a");
        let mut session = EditSession::new(&note);
        assert!(session.add_tag(" 工作 ", 1));
        assert!(!session.add_tag("工作", 2));
        assert!(session.add_tag("会议", 3));
        assert_eq!(session.draft().tags, ["工作", "会议"]);
        assert!(session.remove_tag("工作", 4));
        assert_eq!(session.draft().tags, ["会议"]);
    }

    #[test]
    fn the_later_save_conflicts_and_the_body_stays_in_the_draft() {
        let temp = TempDir::new();
        let notes = open(temp.path());
        let note = create(&notes, "原文");
        let mut panel = EditSession::new(&note);
        let mut float = EditSession::new(&note);
        panel.set_body("面板改的", 1);
        float.set_body("悬浮改的", 2);
        assert_eq!(panel.save(&notes), SaveOutcome::Saved);
        assert_eq!(float.save(&notes), SaveOutcome::Problem);
        assert_eq!(float.problem(), Some(&SaveProblem::Conflict));
        assert_eq!(float.draft().body, "悬浮改的");
        assert_eq!(notes.get(&note.id).unwrap().body, "面板改的");
        assert_eq!(float.autosave_wait_ms(99_999), None, "冲突时不自动保存");
    }

    #[test]
    fn keep_mine_overwrites_with_the_draft_on_top_of_the_latest_revision() {
        let temp = TempDir::new();
        let notes = open(temp.path());
        let note = create(&notes, "原文");
        let mut first = EditSession::new(&note);
        let mut second = EditSession::new(&note);
        first.set_body("甲", 1);
        second.set_body("乙", 2);
        first.save(&notes);
        second.save(&notes);
        assert_eq!(second.keep_mine(&notes), SaveOutcome::Saved);
        assert_eq!(second.problem(), None);
        let stored = notes.get(&note.id).unwrap();
        assert_eq!(stored.body, "乙");
        assert_eq!(stored.revision, note.revision + 2);
        assert_eq!(second.revision(), stored.revision);
    }

    #[test]
    fn keep_saved_drops_the_draft_and_loads_the_disk_version() {
        let temp = TempDir::new();
        let notes = open(temp.path());
        let note = create(&notes, "原文");
        let mut first = EditSession::new(&note);
        let mut second = EditSession::new(&note);
        first.set_body("甲", 1);
        second.set_body("乙", 2);
        first.save(&notes);
        second.save(&notes);
        let shown = second.keep_saved(&notes).unwrap();
        assert_eq!(shown.body, "甲");
        assert_eq!(second.draft().body, "甲");
        assert!(!second.is_dirty());
        assert_eq!(second.problem(), None);
        assert_eq!(notes.get(&note.id).unwrap().body, "甲");
        assert_eq!(second.revision(), shown.revision);
    }

    #[test]
    fn a_failed_save_keeps_the_draft_and_discard_goes_back() {
        let temp = TempDir::new();
        let notes = open(temp.path());
        let note = create(&notes, "原文");
        let mut session = EditSession::new(&note);
        session.set_body("没存上", 1);
        let path = temp
            .path()
            .join("data/notes")
            .join(format!("{}.json", note.id));
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert_eq!(session.save(&notes), SaveOutcome::Problem);
        assert!(matches!(session.problem(), Some(SaveProblem::Failed(_))));
        assert_eq!(session.draft().body, "没存上");
        assert!(session.is_dirty());
        assert_eq!(
            session.autosave_wait_ms(1),
            Some(1_000),
            "下次编辑后仍会尝试保存"
        );
        session.discard();
        assert_eq!(session.draft().body, "原文");
        assert_eq!(session.problem(), None);
        assert!(!session.is_dirty());
    }

    #[test]
    fn adopt_if_clean_only_replaces_a_clean_session() {
        let temp = TempDir::new();
        let notes = open(temp.path());
        let note = create(&notes, "原文");
        let mut other = EditSession::new(&note);
        other.set_body("别处", 1);
        other.save(&notes);
        let latest = notes.get(&note.id).unwrap();

        let mut clean = EditSession::new(&note);
        assert!(clean.adopt_if_clean(&latest));
        assert_eq!(clean.draft().body, "别处");
        assert!(!clean.adopt_if_clean(&latest));

        let mut dirty = EditSession::new(&note);
        dirty.set_body("我的", 2);
        assert!(!dirty.adopt_if_clean(&latest));
        assert_eq!(dirty.draft().body, "我的");
    }
}
