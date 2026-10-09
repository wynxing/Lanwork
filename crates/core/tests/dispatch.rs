//! 查询调度。界面不在这里。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lanwork_core::apps::{AppEntry, AppHit, AppSource, LaunchTarget};
use lanwork_core::dispatch::{
    AppLookup, Dispatch, DispatchError, FILE_QUERY_DELAY_MS, FileLookup, FileProgress, IconKey,
    IconLoader, MissingIcons, Monotonic, QueryView, RgbaImage, RowDetail, SearchGroup, SearchRow,
    Services, Sides, Surface, UsageKey, ViewPhase,
};
use lanwork_core::files::{
    EverythingStatus, FILE_INDEX_UNAVAILABLE, FileHit, FileKind, FileQueryResult, FileSource,
    WindowsSearchStatus,
};
use lanwork_core::notes::{EMPTY_TITLE_DISPLAY, NoteCommands, NoteInput};
use lanwork_core::search::HitKind;
use lanwork_core::storage::{DocumentId, Store, StorePaths};
use lanwork_core::todos::{NewTodo, TodoCommands};

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("lanwork-dispatch-{nanos}-{seq}"));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct ManualClock(Arc<AtomicU64>);

impl Monotonic for ManualClock {
    fn now_ns(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct NoApps;

impl AppLookup for NoApps {
    fn query(&mut self, _text: &str) -> Vec<AppHit> {
        Vec::new()
    }
}

struct EchoApps;

impl AppLookup for EchoApps {
    fn query(&mut self, text: &str) -> Vec<AppHit> {
        if text.chars().any(|ch| !ch.is_whitespace()) {
            vec![app_named(text, HitKind::Prefix)]
        } else {
            Vec::new()
        }
    }
}

struct WxApps;

impl AppLookup for WxApps {
    fn query(&mut self, text: &str) -> Vec<AppHit> {
        if text == "wx" {
            vec![app_named("wx", HitKind::Prefix)]
        } else {
            Vec::new()
        }
    }
}

struct ScriptFiles {
    calls: Arc<Mutex<Vec<(u64, String)>>>,
    result: FileQueryResult,
}

impl FileLookup for ScriptFiles {
    fn query(&mut self, sequence: u64, text: &str) -> FileQueryResult {
        self.calls
            .lock()
            .expect("calls")
            .push((sequence, text.to_owned()));
        let mut result = self.result.clone();
        result.sequence = sequence;
        result
    }
}

struct ChanFiles {
    started: std::sync::mpsc::Sender<(u64, String)>,
    result: std::sync::mpsc::Receiver<FileQueryResult>,
}

impl FileLookup for ChanFiles {
    fn query(&mut self, sequence: u64, text: &str) -> FileQueryResult {
        let _ = self.started.send((sequence, text.to_owned()));
        match self.result.recv_timeout(Duration::from_secs(2)) {
            Ok(mut result) => {
                result.sequence = sequence;
                result
            }
            Err(_) => file_result(FileSource::Unavailable, Vec::new()),
        }
    }
}

struct CountIcons {
    loads: Arc<AtomicUsize>,
}

impl IconLoader for CountIcons {
    fn load(&mut self, _key: &IconKey) -> Option<RgbaImage> {
        self.loads.fetch_add(1, Ordering::SeqCst);
        RgbaImage::new(1, 1, vec![1, 2, 3, 255])
    }
}

fn open_store() -> (TempDir, Store) {
    let temp = TempDir::new();
    let root = temp.path();
    let store = Store::open(StorePaths {
        data_dir: root.join("data"),
        cache_dir: root.join("cache"),
        user_profile: root.join("profile"),
        local_app_data: root.join("local"),
    })
    .unwrap();
    (temp, store)
}

fn booted(store: &Store) -> Services {
    let todos = TodoCommands::open(store.clone());
    todos.boot().unwrap();
    let notes = NoteCommands::open(store.clone()).unwrap();
    Services { todos, notes }
}

fn dispatch<A, F, I>(
    store: Store,
    services: Services,
    apps: A,
    files: F,
    icons: I,
) -> (Dispatch, Arc<AtomicU64>)
where
    A: AppLookup + 'static,
    F: FileLookup + 'static,
    I: IconLoader + 'static,
{
    let now = Arc::new(AtomicU64::new(0));
    let dispatch = Dispatch::new(
        store,
        services,
        Sides {
            apps,
            files,
            clock: ManualClock(Arc::clone(&now)),
            icons,
        },
    );
    (dispatch, now)
}

fn ready<A, F>(store: Store, apps: A, files: F) -> (Dispatch, Arc<AtomicU64>, Services)
where
    A: AppLookup + 'static,
    F: FileLookup + 'static,
{
    let services = booted(&store);
    // 夹具继续写的是调度器里的那一份内存。再 `open` 会得到另一份未加载的服务。
    let kept = Services {
        todos: services.todos.clone(),
        notes: services.notes.clone(),
    };
    let (dispatch, now) = dispatch(store, services, apps, files, MissingIcons);
    dispatch.build().unwrap();
    (dispatch, now, kept)
}

fn app_named(name: &str, kind: HitKind) -> AppHit {
    AppHit {
        entry: AppEntry {
            name: name.to_owned(),
            source: AppSource::Path,
            target: LaunchTarget::Path {
                path: PathBuf::from(format!(r"C:\apps\{name}.exe")),
                args: String::new(),
                working_directory: None,
            },
            icon_path: None,
            icon_index: 0,
            alternate_names: Vec::new(),
        },
        kind,
        score: 1,
    }
}

fn file_result(source: FileSource, hits: Vec<FileHit>) -> FileQueryResult {
    let (everything, windows_search) = match source {
        FileSource::Everything => (EverythingStatus::Ready, WindowsSearchStatus::NotChecked),
        FileSource::WindowsSearch => (EverythingStatus::NotRunning, WindowsSearchStatus::Available),
        FileSource::Unavailable => (
            EverythingStatus::NotRunning,
            WindowsSearchStatus::Unavailable,
        ),
        FileSource::Blank => (
            EverythingStatus::NotChecked,
            WindowsSearchStatus::NotChecked,
        ),
    };
    FileQueryResult {
        sequence: 0,
        everything,
        windows_search,
        source,
        hits,
    }
}

fn file_hit(name: &str, kind: FileKind) -> FileHit {
    FileHit {
        name: name.to_owned(),
        path: format!(r"C:\docs\{name}"),
        kind,
    }
}

fn new_todo(title: &str) -> NewTodo {
    NewTodo {
        title: title.to_owned(),
        due: None,
        remind_at: None,
        recurrence: None,
        source: None,
    }
}

fn labels(view: &QueryView) -> Vec<&str> {
    view.rows.iter().map(|row| row.label.as_str()).collect()
}

fn groups(view: &QueryView) -> Vec<SearchGroup> {
    view.rows.iter().map(|row| row.group).collect()
}

type FileCalls = Arc<Mutex<Vec<(u64, String)>>>;

fn script(result: FileQueryResult) -> (ScriptFiles, FileCalls) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    (
        ScriptFiles {
            calls: Arc::clone(&calls),
            result,
        },
        calls,
    )
}

#[test]
fn index_is_refused_before_boot() {
    let (_temp, store) = open_store();
    let todos = TodoCommands::open(store.clone());
    let notes = NoteCommands::open(store.clone()).unwrap();
    let (dispatch, _) = dispatch(
        store,
        Services { todos, notes },
        NoApps,
        ScriptFiles {
            calls: Arc::new(Mutex::new(Vec::new())),
            result: file_result(FileSource::Everything, Vec::new()),
        },
        MissingIcons,
    );
    assert!(matches!(dispatch.build(), Err(DispatchError::NotReady)));
    assert!(matches!(
        dispatch.submit(Surface::SearchBar, "wx"),
        Err(DispatchError::NotReady)
    ));
}

#[test]
fn local_results_return_before_the_file_request() {
    let (_temp, store) = open_store();
    let (files, calls) = script(file_result(
        FileSource::Everything,
        vec![file_hit("wx.txt", FileKind::File)],
    ));
    let (dispatch, now, _) = ready(store, WxApps, files);
    let view = dispatch.submit(Surface::SearchBar, "wx").unwrap();
    assert_eq!(labels(&view), vec!["wx"]);
    assert_eq!(groups(&view), vec![SearchGroup::Application]);
    assert_eq!(view.file, FileProgress::Waiting);
    assert!(calls.lock().expect("calls").is_empty());

    now.store(FILE_QUERY_DELAY_MS * 1_000_000 - 1, Ordering::SeqCst);
    let early = dispatch.poll().unwrap();
    assert!(!early.sent);
    assert!(calls.lock().expect("calls").is_empty());
    assert_eq!(early.view.file, FileProgress::Waiting);
    assert_eq!(labels(&early.view), vec!["wx"]);

    now.store(FILE_QUERY_DELAY_MS * 1_000_000, Ordering::SeqCst);
    let merged = dispatch.poll().unwrap();
    assert!(merged.sent);
    assert!(merged.accepted);
    assert_eq!(merged.view.file, FileProgress::Settled);
    assert_eq!(
        groups(&merged.view),
        vec![SearchGroup::Application, SearchGroup::File]
    );
    assert_eq!(calls.lock().expect("calls").len(), 1);
    let local = dispatch
        .latency_records()
        .into_iter()
        .find(|record| record.source == Some("local"))
        .unwrap();
    let file = dispatch
        .latency_records()
        .into_iter()
        .find(|record| record.source == Some("everything"))
        .unwrap();
    assert_eq!(local.start_ns, 0);
    assert_eq!(file.start_ns, local.start_ns);
    assert_eq!(file.seq, local.seq);
    assert!(!file.superseded);
}

#[test]
fn file_rows_appear_only_after_the_query_returns() {
    let (_temp, store) = open_store();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let services = booted(&store);
    let (dispatch, now) = dispatch(
        store,
        services,
        WxApps,
        ChanFiles {
            started: started_tx,
            result: result_rx,
        },
        MissingIcons,
    );
    dispatch.build().unwrap();
    let view = dispatch.submit(Surface::SearchBar, "wx").unwrap();
    let sequence = view.sequence.unwrap();
    now.store(FILE_QUERY_DELAY_MS * 1_000_000, Ordering::SeqCst);
    let dispatch = Arc::new(dispatch);
    let worker = {
        let dispatch = Arc::clone(&dispatch);
        std::thread::spawn(move || dispatch.poll().unwrap())
    };
    let (sent_seq, sent_text) = started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(sent_text, "wx");
    assert_eq!(sent_seq, sequence);
    let mid = dispatch.view();
    assert_eq!(mid.file, FileProgress::InFlight);
    assert_eq!(labels(&mid), vec!["wx"]);
    assert!(mid.rows.iter().all(|row| row.group != SearchGroup::File));
    result_tx
        .send(file_result(
            FileSource::Everything,
            vec![file_hit("wx.txt", FileKind::File)],
        ))
        .unwrap();
    let done = worker.join().unwrap();
    assert!(done.accepted);
    assert_eq!(
        groups(&done.view),
        vec![SearchGroup::Application, SearchGroup::File]
    );
}

#[test]
fn rapid_input_keeps_only_the_last_query() {
    let (_temp, store) = open_store();
    let (files, calls) = script(file_result(FileSource::Everything, Vec::new()));
    let (dispatch, now, _) = ready(store, EchoApps, files);
    let text = "abcdefghij";
    let mut first = None;
    let mut last = None;
    for index in 1..=text.len() {
        let view = dispatch.submit(Surface::SearchBar, &text[..index]).unwrap();
        if first.is_none() {
            first = view.sequence;
        }
        last = view.sequence;
        assert_eq!(labels(&view), vec![&text[..index]]);
    }
    let first = first.unwrap();
    let last = last.unwrap();
    assert_ne!(first, last);
    assert!(!dispatch.accepts(first));
    assert!(dispatch.accepts(last));
    assert_eq!(dispatch.view().text, text);
    now.store(FILE_QUERY_DELAY_MS * 1_000_000, Ordering::SeqCst);
    dispatch.poll().unwrap();
    let calls = calls.lock().expect("calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1, text);
    assert_eq!(calls[0].0, last);
    let superseded = dispatch
        .latency_records()
        .iter()
        .filter(|record| record.seq == first && record.superseded)
        .count();
    assert_eq!(superseded, 1);
}

#[test]
fn capture_drops_late_results_and_the_panel_still_searches() {
    let (_temp, store) = open_store();
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (result_tx, result_rx) = std::sync::mpsc::channel();
    let services = booted(&store);
    let (dispatch, now) = dispatch(
        store,
        services,
        EchoApps,
        ChanFiles {
            started: started_tx,
            result: result_rx,
        },
        MissingIcons,
    );
    dispatch.build().unwrap();
    let search = dispatch.submit(Surface::SearchBar, "wx").unwrap();
    let sequence = search.sequence.unwrap();
    now.store(FILE_QUERY_DELAY_MS * 1_000_000, Ordering::SeqCst);
    let dispatch = Arc::new(dispatch);
    let worker = {
        let dispatch = Arc::clone(&dispatch);
        std::thread::spawn(move || dispatch.poll().unwrap())
    };
    let _ = started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let capture = dispatch.submit(Surface::SearchBar, "+ x").unwrap();
    assert_eq!(capture.phase, ViewPhase::Capture);
    assert!(capture.rows.is_empty());
    assert!(!dispatch.accepts(sequence));
    result_tx
        .send(file_result(
            FileSource::Everything,
            vec![file_hit("late.txt", FileKind::File)],
        ))
        .unwrap();
    let late = worker.join().unwrap();
    assert!(!late.accepted);
    assert_eq!(late.view.phase, ViewPhase::Capture);
    assert!(late.view.rows.is_empty());
    assert!(!dispatch.accepts(sequence));

    let again = dispatch.submit(Surface::SearchBar, "wx").unwrap();
    assert_eq!(again.phase, ViewPhase::Results);
    let again_seq = again.sequence.unwrap();
    assert_ne!(again_seq, sequence);
    assert!(dispatch.accepts(again_seq));
    assert!(!dispatch.accepts(sequence));

    let panel = dispatch.submit(Surface::Panel, "+ x").unwrap();
    assert_eq!(panel.phase, ViewPhase::Results);
    assert_eq!(labels(&panel), vec!["+ x"]);
    let note = dispatch.submit(Surface::SearchBar, "/note 标题").unwrap();
    assert_eq!(note.phase, ViewPhase::Capture);
    let wide = dispatch.submit(Surface::SearchBar, "＋ 买牛奶").unwrap();
    assert_eq!(wide.phase, ViewPhase::Capture);
    assert!(!dispatch.accepts(again_seq));
    let notes_word = dispatch.submit(Surface::SearchBar, "/notes").unwrap();
    assert_eq!(notes_word.phase, ViewPhase::Results);
    assert_eq!(labels(&notes_word), vec!["/notes"]);
}

#[test]
fn invalid_urls_do_not_add_a_browser_row() {
    let (_temp, store) = open_store();
    let (files, _) = script(file_result(FileSource::Everything, Vec::new()));
    let (dispatch, _, _) = ready(store, NoApps, files);
    for text in ["htp://x", "http://"] {
        let view = dispatch.submit(Surface::SearchBar, text).unwrap();
        assert!(
            view.rows
                .iter()
                .all(|row| row.group != SearchGroup::Browser),
            "{text}"
        );
    }
    let view = dispatch
        .submit(Surface::SearchBar, "https://example.com/a")
        .unwrap();
    assert_eq!(groups(&view), vec![SearchGroup::Browser]);
    assert_eq!(view.rows[0].label, "https://example.com/a");
    match &view.rows[0].detail {
        RowDetail::Browser { url } => assert_eq!(url, "https://example.com/a"),
        other => panic!("expected browser, {other:?}"),
    }
}

#[test]
fn file_failure_keeps_local_rows() {
    let (_temp, store) = open_store();
    let (files, _) = script(file_result(FileSource::Unavailable, Vec::new()));
    let (dispatch, now, _) = ready(store, WxApps, files);
    dispatch.submit(Surface::SearchBar, "wx").unwrap();
    now.store(FILE_QUERY_DELAY_MS * 1_000_000, Ordering::SeqCst);
    let merged = dispatch.poll().unwrap();
    assert!(merged.accepted);
    assert_eq!(labels(&merged.view), vec!["wx"]);
    assert_eq!(merged.view.file_unavailable, Some(FILE_INDEX_UNAVAILABLE));
    assert!(
        merged
            .view
            .rows
            .iter()
            .all(|row| row.group != SearchGroup::File)
    );
}

#[test]
fn completed_and_trashed_items_stay_out_of_the_index() {
    let (_temp, store) = open_store();
    let (files, _) = script(file_result(FileSource::Everything, Vec::new()));
    let (dispatch, _, services) = ready(store.clone(), NoApps, files);
    let list_id = services.todos.create_list("工作").unwrap();
    let open = services
        .todos
        .create_item(&list_id, new_todo("未完成"))
        .unwrap();
    let done = services
        .todos
        .create_item(&list_id, new_todo("已完成"))
        .unwrap();
    let trashed = services
        .todos
        .create_item(&list_id, new_todo("回收待办"))
        .unwrap();
    services.todos.complete_item(&done).unwrap();
    services.todos.soft_delete(&trashed).unwrap();
    let note = services
        .notes
        .create(&NoteInput {
            title: "还在".to_owned(),
            body: "正文".to_owned(),
            tags: vec!["工作".to_owned()],
            pinned: false,
        })
        .unwrap();
    let deleted = services
        .notes
        .create(&NoteInput {
            title: "删了".to_owned(),
            body: "回收便签".to_owned(),
            tags: Vec::new(),
            pinned: false,
        })
        .unwrap();
    services
        .notes
        .soft_delete(&deleted.id, deleted.revision)
        .unwrap();

    let view = dispatch.submit(Surface::Panel, "未完成").unwrap();
    assert_eq!(labels(&view), vec!["未完成"]);
    assert_eq!(view.rows[0].location, "工作");
    assert!(
        dispatch
            .submit(Surface::Panel, "已完成")
            .unwrap()
            .rows
            .is_empty()
    );
    assert!(
        dispatch
            .submit(Surface::Panel, "回收待办")
            .unwrap()
            .rows
            .is_empty()
    );
    assert!(
        dispatch
            .submit(Surface::Panel, "回收便签")
            .unwrap()
            .rows
            .is_empty()
    );
    assert_eq!(
        labels(&dispatch.submit(Surface::Panel, "还在").unwrap()),
        vec!["还在"]
    );

    services.todos.complete_item(&open).unwrap();
    assert!(
        dispatch
            .submit(Surface::Panel, "未完成")
            .unwrap()
            .rows
            .is_empty()
    );
    let _ = note;
}

#[test]
fn note_body_returns_the_matching_line_and_pinyin_skips_the_body() {
    let (_temp, store) = open_store();
    let (files, _) = script(file_result(FileSource::Everything, Vec::new()));
    let (dispatch, _, services) = ready(store, NoApps, files);
    services
        .notes
        .create(&NoteInput {
            title: "微信".to_owned(),
            body: "第一行\n命中这一行\n第三行".to_owned(),
            tags: vec!["工作".to_owned()],
            pinned: false,
        })
        .unwrap();
    services
        .notes
        .create(&NoteInput {
            title: " ".to_owned(),
            body: "工作".to_owned(),
            tags: Vec::new(),
            pinned: false,
        })
        .unwrap();

    let initials = dispatch.submit(Surface::Panel, "wx").unwrap();
    assert_eq!(labels(&initials), vec!["微信"]);
    assert_eq!(initials.rows[0].kind, Some(HitKind::Initial));
    assert!(initials.rows[0].location.is_empty());
    let pinyin = dispatch.submit(Surface::Panel, "weixin").unwrap();
    assert_eq!(labels(&pinyin), vec!["微信"]);
    assert_eq!(pinyin.rows[0].kind, Some(HitKind::Pinyin));
    assert!(pinyin.rows[0].location.is_empty());

    let line = dispatch.submit(Surface::Panel, "命中").unwrap();
    assert_eq!(line.rows.len(), 1);
    assert_eq!(line.rows[0].location, "命中这一行");
    assert_eq!(line.rows[0].kind, Some(HitKind::Substring));

    let tag = dispatch.submit(Surface::Panel, "gz").unwrap();
    assert_eq!(labels(&tag), vec!["微信"]);

    let body_only = dispatch.submit(Surface::Panel, "gz").unwrap();
    assert!(
        body_only
            .rows
            .iter()
            .all(|row| row.label != EMPTY_TITLE_DISPLAY)
    );
    let empty_title = dispatch.submit(Surface::Panel, "工作").unwrap();
    assert!(
        empty_title
            .rows
            .iter()
            .any(|row| row.label == EMPTY_TITLE_DISPLAY && row.location == "工作")
    );
}

#[test]
fn same_kind_orders_by_use_count_and_exact_stays_first() {
    let (_temp, store) = open_store();
    let (files, _) = script(file_result(FileSource::Everything, Vec::new()));
    let (dispatch, _, services) = ready(store, NoApps, files);
    let list_id = services.todos.create_list("清单").unwrap();
    let wx = services
        .todos
        .create_item(&list_id, new_todo("wx"))
        .unwrap();
    let word = services
        .todos
        .create_item(&list_id, new_todo("word"))
        .unwrap();
    let wxed = services
        .todos
        .create_item(&list_id, new_todo("wxed"))
        .unwrap();

    let prefix = dispatch.submit(Surface::Panel, "w").unwrap();
    assert_eq!(labels(&prefix), vec!["wx", "word", "wxed"]);
    dispatch.record_open(&UsageKey::Todo(word.clone()));
    assert_eq!(dispatch.usage_count(&UsageKey::Todo(word.clone())), 1);
    let resorted = dispatch.submit(Surface::Panel, "w").unwrap();
    assert_eq!(labels(&resorted), vec!["word", "wx", "wxed"]);

    let exact = dispatch.submit(Surface::Panel, "wx").unwrap();
    assert_eq!(labels(&exact), vec!["wx", "wxed"]);
    assert_eq!(exact.rows[0].kind, Some(HitKind::Exact));
    for _ in 0..4 {
        dispatch.record_open(&UsageKey::Todo(wxed.clone()));
    }
    let still = dispatch.submit(Surface::Panel, "wx").unwrap();
    assert_eq!(labels(&still), vec!["wx", "wxed"]);
    assert_eq!(still.rows[0].kind, Some(HitKind::Exact));
    let _ = wx;
}

#[test]
fn query_does_not_read_the_file_and_follows_memory_updates() {
    let (_temp, store) = open_store();
    let (files, _) = script(file_result(FileSource::Everything, Vec::new()));
    let (dispatch, _, services) = ready(store.clone(), NoApps, files);
    let list_id = services.todos.create_list("清单").unwrap();
    let item_id = services
        .todos
        .create_item(&list_id, new_todo("磁盘旧标题"))
        .unwrap();
    assert_eq!(
        labels(&dispatch.submit(Surface::Panel, "磁盘旧标题").unwrap()),
        vec!["磁盘旧标题"]
    );
    let path = store
        .document_path(&DocumentId::Todo(list_id.clone()))
        .unwrap();
    let raw = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, raw.replace("磁盘旧标题", "磁盘新标题")).unwrap();
    assert_eq!(
        labels(&dispatch.submit(Surface::Panel, "磁盘旧标题").unwrap()),
        vec!["磁盘旧标题"]
    );
    assert!(
        dispatch
            .submit(Surface::Panel, "磁盘新标题")
            .unwrap()
            .rows
            .is_empty()
    );
    services.todos.rename_item(&item_id, "磁盘新标题").unwrap();
    assert!(
        dispatch
            .submit(Surface::Panel, "磁盘旧标题")
            .unwrap()
            .rows
            .is_empty()
    );
    assert_eq!(
        labels(&dispatch.submit(Surface::Panel, "磁盘新标题").unwrap()),
        vec!["磁盘新标题"]
    );
}

#[test]
fn interrupted_move_indexes_one_todo() {
    let (_temp, store) = open_store();
    write_todo(
        &store,
        "a",
        r#"{"schemaVersion":1,"id":"a","name":"源","kind":"normal","items":[{"id":"item","title":"旧","movedAt":1}]}"#,
    );
    write_todo(
        &store,
        "b",
        r#"{"schemaVersion":1,"id":"b","name":"目标","kind":"normal","items":[{"id":"item","title":"新","movedAt":2}]}"#,
    );
    let services = booted(&store);
    let (dispatch, _) = dispatch(
        store,
        services,
        NoApps,
        ScriptFiles {
            calls: Arc::new(Mutex::new(Vec::new())),
            result: file_result(FileSource::Everything, Vec::new()),
        },
        MissingIcons,
    );
    dispatch.build().unwrap();
    let ids = dispatch.resident_todo_ids();
    assert_eq!(ids, vec!["item".to_owned()]);
    assert_eq!(
        labels(&dispatch.submit(Surface::Panel, "新").unwrap()),
        vec!["新"]
    );
    assert!(
        dispatch
            .submit(Surface::Panel, "旧")
            .unwrap()
            .rows
            .is_empty()
    );
}

#[test]
fn display_limit_and_icons_cover_only_visible_rows() {
    let (_temp, store) = open_store();
    let hits = (0..25)
        .map(|index| FileHit {
            name: format!("f{index}"),
            path: format!(r"C:\docs\f{index}.txt"),
            kind: FileKind::File,
        })
        .collect();
    let (files, _) = script(file_result(FileSource::Everything, hits));
    let loads = Arc::new(AtomicUsize::new(0));
    let services = booted(&store);
    let now = Arc::new(AtomicU64::new(0));
    let dispatch = Dispatch::new(
        store,
        services,
        Sides {
            apps: WxApps,
            files,
            clock: ManualClock(Arc::clone(&now)),
            icons: CountIcons {
                loads: Arc::clone(&loads),
            },
        },
    );
    dispatch.build().unwrap();
    dispatch.submit(Surface::SearchBar, "wx").unwrap();
    now.store(FILE_QUERY_DELAY_MS * 1_000_000, Ordering::SeqCst);
    let merged = dispatch.poll().unwrap();
    assert_eq!(merged.view.rows.len(), 20);
    assert_eq!(
        merged
            .view
            .rows
            .iter()
            .filter(|row| row.group == SearchGroup::File)
            .count(),
        19
    );
    let loaded = dispatch.load_visible_icons();
    assert_eq!(loaded.len(), 20);
    assert_eq!(loads.load(Ordering::SeqCst), 20);
    assert!(
        merged
            .view
            .rows
            .iter()
            .any(|row: &SearchRow| row.icon.is_some())
    );
}

#[test]
fn file_and_folder_groups_keep_source_order_inside_each_group() {
    let (_temp, store) = open_store();
    let (files, _) = script(file_result(
        FileSource::WindowsSearch,
        vec![
            file_hit("b-folder", FileKind::Folder),
            file_hit("a.txt", FileKind::File),
            file_hit("c-folder", FileKind::Folder),
        ],
    ));
    let (dispatch, now, _) = ready(store, NoApps, files);
    dispatch.submit(Surface::Panel, "folder").unwrap();
    now.store(FILE_QUERY_DELAY_MS * 1_000_000, Ordering::SeqCst);
    let view = dispatch.poll().unwrap().view;
    assert_eq!(
        groups(&view),
        vec![SearchGroup::File, SearchGroup::Folder, SearchGroup::Folder]
    );
    assert_eq!(labels(&view), vec!["a.txt", "b-folder", "c-folder"]);
    let record = dispatch
        .latency_records()
        .into_iter()
        .find(|record| record.source == Some("windows_search"))
        .unwrap();
    assert!(dispatch.mark_rendered(record.seq, "windows_search", record.start_ns + 80));
    let line = dispatch
        .latency_records()
        .into_iter()
        .find(|item| item.source == Some("windows_search"))
        .unwrap()
        .to_json_line();
    assert!(line.contains("\"source\":\"windows_search\""));
    assert!(line.contains("\"end_ns\":80") || line.contains("\"end_ns\": 80"));
}

fn write_todo(store: &Store, id: &str, body: &str) {
    let path = store.document_path(&DocumentId::Todo(id.into())).unwrap();
    std::fs::write(path, body).unwrap();
}
