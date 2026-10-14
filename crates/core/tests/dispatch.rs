//! 查询调度。界面不在这里。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use lanwork_core::apps::{
    AppEntry, AppHit, AppSource, HiddenApp, LaunchTarget, UserCatalog, load_user_catalog,
};
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
use lanwork_core::search::{HitKind, WebSearchEngine};
use lanwork_core::shelves::{IncomingRef, IncomingWeb, ShelfCommands};
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
    Services {
        todos,
        notes,
        shelves: None,
    }
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
        shelves: None,
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
            source_path: None,
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

/// 网页搜索那一条之外的结果。网页搜索单独测。
fn results(view: &QueryView) -> impl Iterator<Item = &SearchRow> {
    view.rows
        .iter()
        .filter(|row| row.group != SearchGroup::WebSearch)
}

fn labels(view: &QueryView) -> Vec<&str> {
    results(view).map(|row| row.label.as_str()).collect()
}

fn groups(view: &QueryView) -> Vec<SearchGroup> {
    results(view).map(|row| row.group).collect()
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
        Services {
            todos,
            notes,
            shelves: None,
        },
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

struct StoreAndPathApps;

impl AppLookup for StoreAndPathApps {
    fn query(&mut self, _text: &str) -> Vec<AppHit> {
        let mut store = app_named("ChatGPT", HitKind::Prefix);
        store.entry.source = AppSource::Store;
        store.entry.target = LaunchTarget::Aumid {
            aumid: "OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0!ChatGPT".to_owned(),
        };
        vec![store, app_named("Chrome", HitKind::Prefix)]
    }
}

#[test]
fn store_apps_take_their_icon_from_the_apps_folder_item() {
    let (_temp, store) = open_store();
    let (files, _) = script(file_result(FileSource::Everything, Vec::new()));
    let (dispatch, _now, _) = ready(store, StoreAndPathApps, files);
    let view = dispatch.submit(Surface::SearchBar, "ch").unwrap();
    let icons: Vec<_> = view.rows.iter().map(|row| row.icon.clone()).collect();
    assert_eq!(
        icons,
        vec![
            Some(IconKey {
                path: r"shell:AppsFolder\OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0!ChatGPT".to_owned(),
                index: 0,
            }),
            Some(IconKey {
                path: r"C:\apps\Chrome.exe".to_owned(),
                index: 0,
            }),
            None,
        ]
    );
}

#[test]
fn web_search_is_always_the_last_row_while_there_is_input() {
    let (_temp, store) = open_store();
    let (files, _) = script(file_result(FileSource::Everything, Vec::new()));
    let (dispatch, _now, _) = ready(store, EchoApps, files);

    let view = dispatch.submit(Surface::SearchBar, " chat ").unwrap();
    let last = view.rows.last().unwrap();
    assert_eq!(last.group, SearchGroup::WebSearch);
    assert_eq!(last.label, "搜索 Google：chat");
    assert_eq!(last.usage, UsageKey::WebSearch);
    assert_eq!(
        last.detail,
        RowDetail::WebSearch {
            url: "https://www.google.com/search?q=chat".to_owned()
        }
    );

    let url = dispatch
        .submit(Surface::SearchBar, "https://example.com/a")
        .unwrap();
    assert_eq!(url.rows.first().unwrap().group, SearchGroup::Browser);
    assert_eq!(url.rows.last().unwrap().group, SearchGroup::WebSearch);

    dispatch.set_web_search_engine(WebSearchEngine::Bing);
    let bing = dispatch.submit(Surface::Panel, "微信").unwrap();
    assert_eq!(bing.rows.last().unwrap().label, "搜索必应：微信");

    assert!(
        dispatch
            .submit(Surface::SearchBar, "")
            .unwrap()
            .rows
            .is_empty()
    );
    assert!(
        dispatch
            .submit(Surface::SearchBar, "+ 买菜")
            .unwrap()
            .rows
            .is_empty()
    );
    assert!(
        dispatch
            .submit(Surface::Panel, "   ")
            .unwrap()
            .rows
            .is_empty()
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
fn rendered_results_are_not_superseded_by_later_input() {
    let (_temp, store) = open_store();
    let (files, _calls) = script(file_result(FileSource::Everything, Vec::new()));
    let (dispatch, _now, _) = ready(store, EchoApps, files);
    let first = dispatch
        .submit(Surface::SearchBar, "ab")
        .unwrap()
        .sequence
        .unwrap();
    assert!(dispatch.mark_rendered(first, "local", 7));
    dispatch.submit(Surface::SearchBar, "abc").unwrap();
    dispatch.submit(Surface::SearchBar, "").unwrap();
    let records = dispatch.latency_records();
    let local = records
        .iter()
        .find(|record| record.seq == first && record.source == Some("local"))
        .unwrap();
    assert_eq!(local.end_ns, Some(7));
    assert!(!local.superseded);
    let second = records
        .iter()
        .find(|record| record.seq != first && record.source == Some("local"))
        .unwrap();
    assert!(second.superseded);
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
    assert_eq!(view.rows[0].label, "打开网址");
    assert!(view.rows[0].location.is_empty());
    match &view.rows[0].detail {
        RowDetail::Browser { url } => assert_eq!(url, "https://example.com/a"),
        other => panic!("expected browser, {other:?}"),
    }
    assert_eq!(view.rows.last().unwrap().group, SearchGroup::WebSearch);
}

#[test]
fn website_like_input_opens_before_web_search_on_both_surfaces() {
    let (_temp, store) = open_store();
    let (files, _) = script(file_result(FileSource::Everything, Vec::new()));
    let (dispatch, _, _) = ready(store, NoApps, files);

    let bare = dispatch.submit(Surface::SearchBar, " cursor.com ").unwrap();
    assert_eq!(bare.rows[0].label, "打开网址");
    assert_eq!(bare.rows[0].group, SearchGroup::Browser);
    assert_eq!(
        bare.rows[0].usage,
        UsageKey::Browser("https://cursor.com".to_owned())
    );
    match &bare.rows[0].detail {
        RowDetail::Browser { url } => assert_eq!(url, "https://cursor.com"),
        other => panic!("expected browser, {other:?}"),
    }
    let last = bare.rows.last().unwrap();
    assert_eq!(last.group, SearchGroup::WebSearch);
    assert_eq!(last.label, "搜索 Google：cursor.com");

    let path = dispatch
        .submit(Surface::Panel, "https://cursor.com/dashboard")
        .unwrap();
    match &path.rows[0].detail {
        RowDetail::Browser { url } => assert_eq!(url, "https://cursor.com/dashboard"),
        other => panic!("expected browser, {other:?}"),
    }
    assert_eq!(path.rows[0].label, "打开网址");
    assert_eq!(path.rows.last().unwrap().group, SearchGroup::WebSearch);

    let http = dispatch
        .submit(Surface::Panel, "http://cursor.com")
        .unwrap();
    match &http.rows[0].detail {
        RowDetail::Browser { url } => assert_eq!(url, "http://cursor.com"),
        other => panic!("expected browser, {other:?}"),
    }

    // 「名称.后缀」是否打开仍待定。当前会给出这一条。
    let file_name = dispatch.submit(Surface::Panel, "readme.md").unwrap();
    assert_eq!(file_name.rows[0].label, "打开网址");
    match &file_name.rows[0].detail {
        RowDetail::Browser { url } => assert_eq!(url, "https://readme.md"),
        other => panic!("expected browser, {other:?}"),
    }

    for text in ["cursor .com", "file:///C:/Windows", "readme"] {
        let view = dispatch.submit(Surface::SearchBar, text).unwrap();
        assert!(
            view.rows
                .iter()
                .all(|row| row.group != SearchGroup::Browser),
            "{text}"
        );
        assert_eq!(view.rows.last().unwrap().group, SearchGroup::WebSearch);
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
            .iter()
            .all(|row| row.group == SearchGroup::WebSearch)
    );
    assert!(
        dispatch
            .submit(Surface::Panel, "回收待办")
            .unwrap()
            .rows
            .iter()
            .all(|row| row.group == SearchGroup::WebSearch)
    );
    assert!(
        dispatch
            .submit(Surface::Panel, "回收便签")
            .unwrap()
            .rows
            .iter()
            .all(|row| row.group == SearchGroup::WebSearch)
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
            .iter()
            .all(|row| row.group == SearchGroup::WebSearch)
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
    assert_eq!(labels(&line).len(), 1);
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
            .iter()
            .all(|row| row.group == SearchGroup::WebSearch)
    );
    services.todos.rename_item(&item_id, "磁盘新标题").unwrap();
    assert!(
        dispatch
            .submit(Surface::Panel, "磁盘旧标题")
            .unwrap()
            .rows
            .iter()
            .all(|row| row.group == SearchGroup::WebSearch)
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
            .iter()
            .all(|row| row.group == SearchGroup::WebSearch)
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
    assert_eq!(merged.view.rows.len(), 21);
    assert_eq!(
        merged.view.rows.last().map(|row| row.group),
        Some(SearchGroup::WebSearch)
    );
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

struct ListedApps {
    hidden: Arc<Mutex<Vec<HiddenApp>>>,
}

impl AppLookup for ListedApps {
    fn query(&mut self, _text: &str) -> Vec<AppHit> {
        Vec::new()
    }

    fn hide(&mut self, key: &str) -> Result<(), String> {
        if key.is_empty() {
            return Err("不在索引里".to_owned());
        }
        let mut hidden = self.hidden.lock().expect("hidden");
        if hidden.iter().any(|item| item.launch_key == key) {
            return Ok(());
        }
        hidden.push(HiddenApp {
            launch_key: key.to_owned(),
            name: "记事本".to_owned(),
        });
        Ok(())
    }

    fn restore(&mut self, key: &str) -> bool {
        let mut hidden = self.hidden.lock().expect("hidden");
        let before = hidden.len();
        hidden.retain(|item| item.launch_key != key);
        hidden.len() != before
    }

    fn hidden_apps(&mut self) -> Vec<HiddenApp> {
        self.hidden.lock().expect("hidden").clone()
    }

    fn user_catalog(&mut self) -> UserCatalog {
        UserCatalog {
            hidden: self.hidden.lock().expect("hidden").clone(),
            ..UserCatalog::default()
        }
    }
}

struct BlankFiles;

impl FileLookup for BlankFiles {
    fn query(&mut self, sequence: u64, _text: &str) -> FileQueryResult {
        FileQueryResult {
            sequence,
            everything: EverythingStatus::NotChecked,
            windows_search: WindowsSearchStatus::NotChecked,
            source: FileSource::Blank,
            hits: Vec::new(),
        }
    }
}

#[test]
fn hiding_an_app_is_written_and_a_failed_write_rolls_memory_back() {
    let (_temp, store) = open_store();
    let hidden = Arc::new(Mutex::new(Vec::new()));
    let (dispatch, _, _) = ready(
        store.clone(),
        ListedApps {
            hidden: Arc::clone(&hidden),
        },
        BlankFiles,
    );
    let key = r"path:C:\Apps\note.exe";
    dispatch.hide_app(key).unwrap();
    let saved = load_user_catalog(&store.user_apps_path()).unwrap();
    assert_eq!(saved.hidden.len(), 1);
    assert_eq!(saved.hidden[0].launch_key, key);
    assert_eq!(saved.hidden[0].name, "记事本");
    assert_eq!(dispatch.hidden_apps()[0].launch_key, key);

    let path = store.user_apps_path();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    let err = dispatch.hide_app(r"path:C:\Apps\other.exe").unwrap_err();
    assert!(err.contains("写入用户应用目录失败"), "{err}");
    assert_eq!(dispatch.hidden_apps().len(), 1, "写盘失败时内存回到隐藏前");
    assert_eq!(hidden.lock().expect("hidden").len(), 1);

    std::fs::remove_dir(&path).unwrap();
    dispatch.restore_hidden_app(key).unwrap();
    assert!(dispatch.hidden_apps().is_empty());
    assert!(
        load_user_catalog(&store.user_apps_path())
            .unwrap()
            .hidden
            .is_empty()
    );

    dispatch.hide_app(key).unwrap();
    std::fs::remove_file(store.user_apps_path()).unwrap();
    std::fs::create_dir(store.user_apps_path()).unwrap();
    assert!(dispatch.restore_hidden_app(key).is_err());
    assert_eq!(
        dispatch.hidden_apps()[0].launch_key,
        key,
        "恢复写盘失败时这一项仍在名单里"
    );
    assert!(dispatch.hide_app("").is_err());
}

#[test]
fn shelf_web_refs_appear_after_notes_and_before_files() {
    let (_temp, store) = open_store();
    let services = booted(&store);
    services
        .notes
        .create(&NoteInput {
            title: "cursor 笔记".to_owned(),
            body: String::new(),
            tags: Vec::new(),
            pinned: false,
        })
        .unwrap();
    let shelves = ShelfCommands::open(store.clone()).unwrap();
    shelves
        .add_web_refs(
            None,
            &[IncomingWeb {
                url: "https://example.com/cursor-only".to_owned(),
                title: "Cursor Status".to_owned(),
            }],
        )
        .unwrap();
    let (files, _) = script(file_result(
        FileSource::Everything,
        vec![file_hit("cursor.txt", FileKind::File)],
    ));
    let (dispatch, now) = dispatch(
        store,
        Services {
            todos: services.todos,
            notes: services.notes,
            shelves: Some(shelves),
        },
        NoApps,
        files,
        MissingIcons,
    );
    dispatch.build().unwrap();
    let view = dispatch.submit(Surface::SearchBar, "cursor").unwrap();
    assert_eq!(groups(&view), vec![SearchGroup::Note, SearchGroup::Shelf]);
    let web = results(&view)
        .find(|row| row.group == SearchGroup::Shelf)
        .unwrap();
    assert_eq!(web.label, "Cursor Status");
    assert_eq!(web.location, "收纳 \u{00B7} 收纳");
    assert!(matches!(
        &web.detail,
        RowDetail::Web { url } if url == "https://example.com/cursor-only"
    ));
    assert_eq!(
        web.usage,
        UsageKey::Web("https://example.com/cursor-only".to_owned())
    );

    now.store(FILE_QUERY_DELAY_MS * 1_000_000, Ordering::SeqCst);
    let merged = dispatch.poll().unwrap();
    assert_eq!(
        groups(&merged.view),
        vec![SearchGroup::Note, SearchGroup::Shelf, SearchGroup::File]
    );

    let by_url = dispatch.submit(Surface::Panel, "cursor-only").unwrap();
    assert_eq!(groups(&by_url), vec![SearchGroup::Shelf]);
    // 「Status」只在标题里，网址是 cursor-only。只索引网址时这条不会出现。
    let by_title = dispatch.submit(Surface::SearchBar, "status").unwrap();
    assert_eq!(groups(&by_title), vec![SearchGroup::Shelf]);
    let titled = results(&by_title).next().unwrap();
    assert_eq!(titled.label, "Cursor Status");
    assert_eq!(titled.kind, Some(HitKind::Substring));
    dispatch.record_open(&web.usage);
    assert_eq!(dispatch.usage_count(&web.usage), 1);
}

/// 网页引用的标题和网址都能命中，同一条只出现一次。
/// 标题走与文件名相同的命中类型；网址只做原文。文件仍只按名称，不按路径。
#[test]
fn shelf_web_page_matches_its_title_and_its_url_once() {
    let (_temp, store) = open_store();
    let services = booted(&store);
    let shelves = ShelfCommands::open(store.clone()).unwrap();
    let group = shelves.create_group("作业").unwrap();
    shelves
        .add_web_refs(
            Some(&group.id),
            &[
                IncomingWeb {
                    url: "https://authserver.dgut.edu.cn/authserver/login".to_owned(),
                    title: "应用中心".to_owned(),
                },
                IncomingWeb {
                    url: "https://portal.dgut.example/home".to_owned(),
                    title: "dgut 门户".to_owned(),
                },
                IncomingWeb {
                    url: "https://library.example/books".to_owned(),
                    title: "图书馆".to_owned(),
                },
                IncomingWeb {
                    url: "https://notes.example/only-url-token".to_owned(),
                    title: String::new(),
                },
            ],
        )
        .unwrap();
    shelves
        .add_refs(
            Some(&group.id),
            &[IncomingRef {
                path: r"C:\dgut\readme.txt".to_owned(),
                folder: false,
            }],
        )
        .unwrap();
    let (dispatch, _) = dispatch(
        store,
        Services {
            todos: services.todos,
            notes: services.notes,
            shelves: Some(shelves),
        },
        NoApps,
        BlankFiles,
        MissingIcons,
    );
    dispatch.build().unwrap();

    let by_title = dispatch.submit(Surface::SearchBar, "应用中心").unwrap();
    let title_hits: Vec<_> = results(&by_title)
        .filter(|row| row.group == SearchGroup::Shelf)
        .collect();
    assert_eq!(title_hits.len(), 1, "标题完全匹配只出现一条");
    assert_eq!(title_hits[0].label, "应用中心");
    assert_eq!(title_hits[0].kind, Some(HitKind::Exact));
    assert_eq!(title_hits[0].location, "收纳 \u{00B7} 作业");
    assert!(matches!(
        &title_hits[0].detail,
        RowDetail::Web { url } if url == "https://authserver.dgut.edu.cn/authserver/login"
    ));

    let by_part = dispatch.submit(Surface::SearchBar, "中心").unwrap();
    let part = shelf_labeled(&by_part, "应用中心");
    assert_eq!(part.kind, Some(HitKind::Substring));

    let by_reading = dispatch
        .submit(Surface::SearchBar, "yingyongzhongxin")
        .unwrap();
    assert_eq!(
        shelf_labeled(&by_reading, "应用中心").kind,
        Some(HitKind::Pinyin)
    );
    let by_initial = dispatch.submit(Surface::SearchBar, "yyzx").unwrap();
    assert_eq!(
        shelf_labeled(&by_initial, "应用中心").kind,
        Some(HitKind::Initial)
    );

    let by_url = dispatch.submit(Surface::SearchBar, "dgut").unwrap();
    let url_hits: Vec<_> = results(&by_url)
        .filter(|row| row.group == SearchGroup::Shelf)
        .map(|row| row.label.as_str())
        .collect();
    assert_eq!(
        url_hits,
        vec!["dgut 门户", "应用中心"],
        "标题前缀排在只命中网址的子串之前，文件路径里的 dgut 不出现"
    );
    assert_eq!(
        shelf_labeled(&by_url, "dgut 门户").kind,
        Some(HitKind::Prefix),
        "标题和网址同时含有 dgut 时只留更前的标题前缀，不另出一条"
    );
    assert_eq!(
        shelf_labeled(&by_url, "应用中心").kind,
        Some(HitKind::Substring)
    );
    assert!(
        results(&by_url).all(|row| row.label != "readme.txt"),
        "文件不按路径匹配"
    );

    let by_name = dispatch.submit(Surface::SearchBar, "readme").unwrap();
    let file = shelf_labeled(&by_name, "readme.txt");
    assert!(matches!(file.detail, RowDetail::File { .. }));
    assert_eq!(file.kind, Some(HitKind::Prefix));
    assert!(
        results(&by_name).all(|row| !matches!(row.detail, RowDetail::Web { .. })),
        "文件名不把网页引用再带出来"
    );

    let by_reading_title = dispatch.submit(Surface::SearchBar, "tushuguan").unwrap();
    assert_eq!(
        shelf_labeled(&by_reading_title, "图书馆").kind,
        Some(HitKind::Pinyin),
        "标题可以按拼音命中，网址本身没有这串字母"
    );
    let literal_url = dispatch.submit(Surface::SearchBar, "books").unwrap();
    assert_eq!(
        shelf_labeled(&literal_url, "图书馆").kind,
        Some(HitKind::Substring)
    );

    let empty_title = dispatch
        .submit(Surface::SearchBar, "only-url-token")
        .unwrap();
    let bare = shelf_labeled(&empty_title, "https://notes.example/only-url-token");
    assert_eq!(bare.kind, Some(HitKind::Substring));

    let shared = dispatch
        .submit(Surface::SearchBar, "https://library.example/books")
        .unwrap();
    let shared_hits: Vec<_> = results(&shared)
        .filter(|row| row.label == "图书馆")
        .collect();
    assert_eq!(shared_hits.len(), 1, "整段网址完全匹配也只出现一条");
    assert_eq!(shared_hits[0].kind, Some(HitKind::Exact));

    // 加粗用展示名称，和搜索条同一函数。标题命中加粗标题；只命中网址时标题不加粗。
    assert_eq!(bold(title_hits[0], "应用"), Some("应用".to_owned()));
    assert_eq!(bold(shelf_labeled(&by_url, "应用中心"), "dgut"), None);
    assert_eq!(
        bold(shelf_labeled(&by_url, "dgut 门户"), "dgut"),
        Some("dgut".to_owned())
    );
    assert_eq!(
        bold(bare, "only-url-token"),
        Some("only-url-token".to_owned())
    );
    assert_eq!(bold(file, "readme"), Some("readme".to_owned()));
    assert_eq!(
        lanwork_core::shell::match_span("收纳 · 作业", "dgut"),
        None,
        "分组名不参与加粗"
    );
}

fn bold(row: &SearchRow, query: &str) -> Option<String> {
    lanwork_core::shell::match_span(&row.label, query)
        .map(|(start, end)| row.label[start..end].to_owned())
}

fn shelf_labeled<'a>(view: &'a QueryView, label: &str) -> &'a SearchRow {
    results(view)
        .find(|row| row.group == SearchGroup::Shelf && row.label == label)
        .unwrap_or_else(|| panic!("收纳结果里没有 {label}"))
}

#[test]
fn shelf_file_is_not_repeated_in_the_file_group() {
    let (_temp, store) = open_store();
    let services = booted(&store);
    let shelves = ShelfCommands::open(store.clone()).unwrap();
    let group = shelves.create_group("单片机").unwrap();
    let path = r"C:\docs\cursor.txt";
    shelves
        .add_refs(
            Some(&group.id),
            &[IncomingRef {
                path: path.to_owned(),
                folder: false,
            }],
        )
        .unwrap();
    shelves
        .add_refs(
            Some(&group.id),
            &[
                IncomingRef {
                    path: r"C:\docs\微信".to_owned(),
                    folder: false,
                },
                IncomingRef {
                    path: r"C:\docs\board".to_owned(),
                    folder: true,
                },
            ],
        )
        .unwrap();
    let (files, _) = script(file_result(
        FileSource::Everything,
        vec![
            file_hit("cursor.txt", FileKind::File),
            file_hit("cursor-extra.txt", FileKind::File),
        ],
    ));
    let (dispatch, now) = dispatch(
        store,
        Services {
            todos: services.todos,
            notes: services.notes,
            shelves: Some(shelves),
        },
        NoApps,
        files,
        MissingIcons,
    );
    dispatch.build().unwrap();
    dispatch.submit(Surface::SearchBar, "cursor").unwrap();
    now.store(FILE_QUERY_DELAY_MS * 1_000_000, Ordering::SeqCst);
    let merged = dispatch.poll().unwrap();
    let shelf: Vec<_> = results(&merged.view)
        .filter(|row| row.group == SearchGroup::Shelf)
        .collect();
    assert_eq!(shelf.len(), 1);
    assert_eq!(shelf[0].label, "cursor.txt");
    assert_eq!(shelf[0].location, "收纳 \u{00B7} 单片机");
    assert!(matches!(
        &shelf[0].detail,
        RowDetail::File { path } if path == r"C:\docs\cursor.txt"
    ));
    let files: Vec<_> = results(&merged.view)
        .filter(|row| row.group == SearchGroup::File)
        .map(|row| row.label.as_str())
        .collect();
    assert_eq!(files, vec!["cursor-extra.txt"]);

    let named = dispatch.submit(Surface::SearchBar, "weixin").unwrap();
    let by_reading = results(&named)
        .find(|row| row.group == SearchGroup::Shelf)
        .unwrap();
    assert_eq!(by_reading.label, "微信");
    assert_eq!(by_reading.kind, Some(HitKind::Pinyin));

    let folder = dispatch.submit(Surface::SearchBar, "board").unwrap();
    let shelf_folder = results(&folder)
        .find(|row| row.group == SearchGroup::Shelf)
        .unwrap();
    assert!(matches!(shelf_folder.detail, RowDetail::Folder { .. }));
    assert_eq!(shelf_folder.location, "收纳 \u{00B7} 单片机");
}

fn write_todo(store: &Store, id: &str, body: &str) {
    let path = store.document_path(&DocumentId::Todo(id.into())).unwrap();
    std::fs::write(path, body).unwrap();
}
