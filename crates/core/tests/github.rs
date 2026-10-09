//! GitHub 服务的验收。筛选仍因快照缺字段而拒绝。
//!
//! 夹具是手写并脱敏的 `gh --json` 数组。不启动本机 `gh`，不访问网络。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use lanwork_core::CivilDate;
use lanwork_core::github::{
    CommandCapture, CommandRunner, FetchFailure, FetchedRepo, GhCallError, GhClient, GhProbe,
    GithubCommands, GithubError, GithubFilter, GithubSettings, PendingTopic, ProcessGh, RepoResult,
    SnapshotItem, SpawnFailure, Watchlist, cache_file_id, parse_issues, parse_pulls,
};
use lanwork_core::notes::{NoteCommands, NoteInput};
use lanwork_core::storage::{DocumentId, EntityKind, Store, StorePaths};
use lanwork_core::todos::{
    NewTodo, Recurrence, RecurrenceRule, SourceKind, TodoCommands, TodoSource,
};

const NOW_MS: i64 = 1_788_220_800_000;

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
        let path = std::env::temp_dir().join(format!("lanwork-github-{nanos}-{seq}"));
        std::fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

struct ScriptGh {
    probe: Mutex<Result<GhProbe, GhCallError>>,
    fetches: Mutex<HashMap<String, Result<FetchedRepo, GhCallError>>>,
    fetch_count: AtomicUsize,
}

impl ScriptGh {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            probe: Mutex::new(Ok(GhProbe::Ready {
                version: "2.62.0".into(),
            })),
            fetches: Mutex::new(HashMap::new()),
            fetch_count: AtomicUsize::new(0),
        })
    }

    fn set_probe(&self, probe: Result<GhProbe, GhCallError>) {
        *self.probe.lock().unwrap() = probe;
    }

    fn set_fetch(&self, repo: &str, result: Result<FetchedRepo, GhCallError>) {
        self.fetches.lock().unwrap().insert(repo.to_owned(), result);
    }
}

impl GhClient for ScriptGh {
    fn probe(&self) -> Result<GhProbe, GhCallError> {
        self.probe.lock().unwrap().clone()
    }

    fn fetch(&self, repo: &str) -> Result<FetchedRepo, GhCallError> {
        self.fetch_count.fetch_add(1, Ordering::Relaxed);
        self.fetches
            .lock()
            .unwrap()
            .get(repo)
            .cloned()
            .unwrap_or(Err(GhCallError::NotFound))
    }
}

struct Env {
    temp: TempDir,
    store: Store,
    todos: TodoCommands,
    script: Arc<ScriptGh>,
    github: GithubCommands,
}

impl Env {
    fn new(repos: &[&str]) -> Self {
        let temp = TempDir::new();
        let paths = StorePaths {
            data_dir: temp.path.join("data"),
            cache_dir: temp.path.join("cache"),
            user_profile: temp.path.join("profile"),
            local_app_data: temp.path.join("local"),
        };
        let store = Store::open(paths).unwrap();
        let todos = TodoCommands::open(store.clone());
        todos.boot().unwrap();
        let script = ScriptGh::new();
        let github = GithubCommands::open(store.clone(), script.clone(), todos.clone());
        if !repos.is_empty() {
            store
                .write_json(
                    &DocumentId::GithubWatchlist,
                    &Watchlist {
                        schema_version: 1,
                        repos: repos.iter().map(|repo| (*repo).to_owned()).collect(),
                        ignored: Vec::new(),
                        pinned: Vec::new(),
                    },
                )
                .unwrap();
        }
        github.load().unwrap();
        Self {
            temp,
            store,
            todos,
            script,
            github,
        }
    }
}

fn settings(stale_days: u32, auto_complete: bool) -> GithubSettings {
    GithubSettings {
        stale_days,
        auto_complete_on_close: auto_complete,
        source_sync: true,
        refresh_interval_ms: 60_000,
    }
}

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/github")
        .join(name);
    std::fs::read(path).unwrap()
}

fn fetched(pulls: &str, issues: &str) -> FetchedRepo {
    FetchedRepo {
        pulls: parse_pulls(&fixture(pulls)).unwrap(),
        issues: parse_issues(&fixture(issues)).unwrap(),
    }
}

fn widget() -> FetchedRepo {
    fetched("widget-prs.json", "widget-issues.json")
}

fn source(kind: SourceKind, repo: &str, number: u64, url: &str) -> TodoSource {
    TodoSource::try_new(kind, url, repo, number).unwrap()
}

fn inbox_item(env: &Env, title: &str, source: Option<TodoSource>) -> String {
    let inbox = env.todos.ensure_inbox().unwrap();
    env.todos
        .create_item(
            &inbox,
            NewTodo {
                title: title.into(),
                due: None,
                remind_at: None,
                recurrence: None,
                source,
            },
        )
        .unwrap()
}

fn item_completed(env: &Env, id: &str) -> bool {
    env.todos.item(id).unwrap().item.completed
}

fn doc_bytes(store: &Store, doc: &DocumentId) -> Vec<u8> {
    std::fs::read(store.document_path(doc).unwrap()).unwrap()
}

fn cache_doc(repo: &str) -> DocumentId {
    DocumentId::GithubCache(cache_file_id(repo).unwrap())
}

fn titles(env: &Env, repo: &str) -> Vec<String> {
    env.github
        .list(NOW_MS, &settings(14, false))
        .unwrap()
        .into_iter()
        .find(|list| list.repo == repo)
        .unwrap()
        .items
        .into_iter()
        .map(|item| item.title)
        .collect()
}

fn assert_no_secrets(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let bytes = std::fs::read(&path).unwrap();
            let text = String::from_utf8_lossy(&bytes);
            for needle in ["ghp_", "github_pat_", "gho_", "GH_TOKEN", "GITHUB_TOKEN"] {
                assert!(
                    !text.contains(needle),
                    "{} contains {needle}",
                    path.display()
                );
            }
        }
    }
}

struct ScriptRunner {
    calls: Mutex<Vec<Vec<String>>>,
    responses: Vec<Result<CommandCapture, SpawnFailure>>,
}

impl CommandRunner for ScriptRunner {
    fn run(&self, args: &[&str]) -> Result<CommandCapture, SpawnFailure> {
        let mut calls = self.calls.lock().unwrap();
        let index = calls.len();
        calls.push(args.iter().map(|arg| (*arg).to_owned()).collect());
        self.responses
            .get(index)
            .cloned()
            .unwrap_or(Err(SpawnFailure::Failed))
    }
}

fn capture(code: i32, stdout: &str, stderr: &str) -> CommandCapture {
    CommandCapture {
        code,
        stdout: stdout.as_bytes().to_vec(),
        stderr: stderr.as_bytes().to_vec(),
    }
}

#[test]
fn load_before_boot_is_not_ready() {
    let temp = TempDir::new();
    let store = Store::open(StorePaths {
        data_dir: temp.path.join("data"),
        cache_dir: temp.path.join("cache"),
        user_profile: temp.path.join("profile"),
        local_app_data: temp.path.join("local"),
    })
    .unwrap();
    let github = GithubCommands::open(store.clone(), ScriptGh::new(), TodoCommands::open(store));
    assert!(matches!(github.load().unwrap_err(), GithubError::NotReady));
}

#[test]
fn refresh_matches_fixture_then_convert_hides_into_inbox() {
    let env = Env::new(&["example/widget"]);
    env.script.set_fetch("example/widget", Ok(widget()));
    let report = env
        .github
        .refresh_all(NOW_MS, &settings(14, false))
        .unwrap();
    assert_eq!(report.refresh_interval_ms, 60_000);
    assert!(report.source_sync);
    assert!(matches!(
        report.repos[0].result,
        RepoResult::Updated {
            fetched_at: NOW_MS,
            ..
        }
    ));
    let list = env.github.list(NOW_MS, &settings(14, false)).unwrap();
    assert_eq!(list.len(), 1);
    assert!(!list[0].offline_cache);
    assert!(list[0].cache_label.is_none());
    assert_eq!(list[0].fetched_at, Some(NOW_MS));
    let items = &list[0].items;
    assert_eq!(items.len(), 3);
    assert_eq!(items[0].title, "Fix the latch");
    assert_eq!(items[0].number, 12);
    assert_eq!(items[0].kind, SourceKind::GithubPr);
    assert!(items[0].stale);
    assert!(!items[0].draft);
    assert_eq!(items[1].title, "Draft notes");
    assert!(items[1].draft);
    assert!(items[1].stale);
    assert_eq!(items[2].title, "Document the panel");
    assert_eq!(items[2].kind, SourceKind::GithubIssue);
    assert!(!items[2].stale);
    assert!(!items[2].draft);
    assert_eq!(
        env.github
            .open_url("example/widget", SourceKind::GithubPr, 12)
            .unwrap(),
        "https://github.com/example/widget/pull/12"
    );

    let before_list = doc_bytes(&env.store, &DocumentId::GithubWatchlist);
    let before_cache = doc_bytes(&env.store, &cache_doc("example/widget"));
    let id = env
        .github
        .convert_to_todo("example/widget", SourceKind::GithubPr, 12)
        .unwrap();
    let stored = env.todos.item(&id).unwrap();
    assert_eq!(stored.list_name, "收件箱");
    assert_eq!(stored.item.title, "example/widget#12 Fix the latch");
    let source = stored.item.source.unwrap();
    assert_eq!(source.kind, SourceKind::GithubPr);
    assert_eq!(source.repo, "example/widget");
    assert_eq!(source.number, 12);
    assert!(!titles(&env, "example/widget").contains(&"Fix the latch".to_owned()));
    assert!(titles(&env, "example/widget").contains(&"Draft notes".to_owned()));
    assert!(
        env.github
            .snapshot("example/widget")
            .unwrap()
            .unwrap()
            .items
            .iter()
            .any(|item| item.number == 12)
    );
    assert_eq!(
        doc_bytes(&env.store, &DocumentId::GithubWatchlist),
        before_list
    );
    assert_eq!(
        doc_bytes(&env.store, &cache_doc("example/widget")),
        before_cache
    );
    assert_no_secrets(&env.temp.path.join("data"));
}

#[test]
fn stale_days_zero_does_not_mark() {
    let env = Env::new(&["example/widget"]);
    env.script.set_fetch("example/widget", Ok(widget()));
    env.github.refresh_all(NOW_MS, &settings(0, false)).unwrap();
    let items = env.github.list(NOW_MS, &settings(0, false)).unwrap()[0]
        .items
        .clone();
    assert!(items.iter().all(|item| !item.stale));
    assert!(items.iter().any(|item| item.draft && item.number == 13));
}

#[test]
fn same_pr_linked_to_several_todos_stays_hidden_until_none_are_open() {
    let env = Env::new(&["example/widget"]);
    env.script.set_fetch("example/widget", Ok(widget()));
    env.github
        .refresh_all(NOW_MS, &settings(14, false))
        .unwrap();
    let pr = source(
        SourceKind::GithubPr,
        "example/widget",
        12,
        "https://github.com/example/widget/pull/12",
    );
    let first = inbox_item(&env, "甲", Some(pr.clone()));
    let second = inbox_item(&env, "乙", Some(pr));
    assert!(!titles(&env, "example/widget").contains(&"Fix the latch".to_owned()));
    let cache = doc_bytes(&env.store, &cache_doc("example/widget"));
    let watch = doc_bytes(&env.store, &DocumentId::GithubWatchlist);
    env.todos.complete_item(&first).unwrap();
    assert!(!titles(&env, "example/widget").contains(&"Fix the latch".to_owned()));
    env.todos.soft_delete(&second).unwrap();
    assert!(titles(&env, "example/widget").contains(&"Fix the latch".to_owned()));
    assert_eq!(doc_bytes(&env.store, &cache_doc("example/widget")), cache);
    assert_eq!(doc_bytes(&env.store, &DocumentId::GithubWatchlist), watch);
}

#[test]
fn empty_snapshot_is_success_and_other_repo_stays() {
    let env = Env::new(&["example/widget", "example/other"]);
    env.script.set_fetch("example/widget", Ok(widget()));
    env.script
        .set_fetch("example/other", Ok(fetched("other-prs.json", "empty.json")));
    env.github
        .refresh_all(NOW_MS, &settings(14, false))
        .unwrap();
    let other_bytes = doc_bytes(&env.store, &cache_doc("example/other"));
    env.script
        .set_fetch("example/widget", Ok(fetched("empty.json", "empty.json")));
    let report = env
        .github
        .refresh_repo("example/widget", NOW_MS + 5, &settings(14, false))
        .unwrap();
    assert!(matches!(
        report.repos[0].result,
        RepoResult::Updated { fetched_at: fetched, .. } if fetched == NOW_MS + 5
    ));
    assert!(titles(&env, "example/widget").is_empty());
    let snapshot = env.github.snapshot("example/widget").unwrap().unwrap();
    assert!(snapshot.items.is_empty());
    assert_eq!(snapshot.fetched_at, NOW_MS + 5);
    let widget_row = env
        .github
        .list(NOW_MS, &settings(14, false))
        .unwrap()
        .into_iter()
        .find(|list| list.repo == "example/widget")
        .unwrap();
    assert!(!widget_row.offline_cache);
    assert_eq!(
        doc_bytes(&env.store, &cache_doc("example/other")),
        other_bytes
    );
    assert_eq!(
        titles(&env, "example/other"),
        vec!["Keep the other repo".to_owned()]
    );
}

#[test]
fn one_repo_failure_keeps_its_snapshot_and_updates_the_other() {
    let env = Env::new(&["example/widget", "example/other", "example/widget"]);
    env.script
        .set_fetch("example/other", Ok(fetched("other-prs.json", "empty.json")));
    env.github
        .refresh_repo("example/other", NOW_MS, &settings(14, false))
        .unwrap();
    let other_bytes = doc_bytes(&env.store, &cache_doc("example/other"));
    env.script.set_fetch("example/widget", Ok(widget()));
    env.script
        .set_fetch("example/other", Err(GhCallError::RateLimit));
    let report = env
        .github
        .refresh_all(NOW_MS + 9, &settings(14, false))
        .unwrap();
    assert_eq!(env.script.fetch_count.load(Ordering::Relaxed), 3);
    assert!(matches!(
        &report.repos[0].result,
        RepoResult::Updated { .. }
    ));
    assert!(matches!(
        &report.repos[1].result,
        RepoResult::Unchanged {
            failure: FetchFailure::RateLimit
        }
    ));
    assert_eq!(report.repos.len(), 2);
    assert_eq!(
        doc_bytes(&env.store, &cache_doc("example/other")),
        other_bytes
    );
    let lists = env.github.list(NOW_MS, &settings(14, false)).unwrap();
    let other = lists
        .iter()
        .find(|list| list.repo == "example/other")
        .unwrap();
    assert!(other.offline_cache);
    assert_eq!(other.cache_label, Some("离线缓存"));
    assert_eq!(other.failure, Some(FetchFailure::RateLimit));
    assert_eq!(other.items[0].title, "Keep the other repo");
    let widget = lists
        .iter()
        .find(|list| list.repo == "example/widget")
        .unwrap();
    assert!(!widget.offline_cache);
    assert_eq!(widget.items.len(), 3);
}

#[test]
fn unavailable_gh_keeps_snapshots_and_does_not_complete() {
    let cases = [
        (
            Ok(GhProbe::NotInstalled),
            FetchFailure::NotInstalled,
            "gh not installed",
        ),
        (
            Ok(GhProbe::NotLoggedIn {
                version: "2.62.0".into(),
            }),
            FetchFailure::NotLoggedIn,
            "gh version 2.62.0",
        ),
        (
            Err(GhCallError::Network),
            FetchFailure::Network,
            "gh unavailable",
        ),
        (
            Err(GhCallError::RateLimit),
            FetchFailure::RateLimit,
            "gh unavailable",
        ),
        (
            Err(GhCallError::NotFound),
            FetchFailure::NotFound,
            "gh unavailable",
        ),
    ];
    for (probe, failure, log_line) in cases {
        let env = Env::new(&["example/widget"]);
        let seeded = lanwork_core::github::RepoSnapshot {
            schema_version: 1,
            repo: "example/widget".into(),
            fetched_at: 10,
            items: vec![SnapshotItem {
                kind: SourceKind::GithubPr,
                number: 12,
                title: "Cached latch".into(),
                url: "https://github.com/example/widget/pull/12".into(),
                updated_at: Some(NOW_MS),
                draft: false,
            }],
        };
        env.store
            .write_json(&cache_doc("example/widget"), &seeded)
            .unwrap();
        env.github.load().unwrap();
        let bytes = doc_bytes(&env.store, &cache_doc("example/widget"));
        let todo = inbox_item(
            &env,
            "关联",
            Some(source(
                SourceKind::GithubIssue,
                "example/widget",
                2,
                "https://github.com/example/widget/issues/2",
            )),
        );
        env.script.set_probe(probe);
        env.script.set_fetch("example/widget", Ok(widget()));
        let events = env.store.subscribe();
        let report = env.github.refresh_all(NOW_MS, &settings(14, true)).unwrap();
        assert!(
            matches!(
                &report.repos[0].result,
                RepoResult::Unchanged { failure: found } if found == &failure
            ),
            "{report:?}"
        );
        assert_eq!(env.script.fetch_count.load(Ordering::Relaxed), 0);
        assert_eq!(doc_bytes(&env.store, &cache_doc("example/widget")), bytes);
        assert!(!item_completed(&env, &todo));
        let row = &env.github.list(NOW_MS, &settings(14, true)).unwrap()[0];
        assert!(row.offline_cache);
        assert_eq!(row.cache_label, Some("离线缓存"));
        assert_eq!(row.items[0].title, "Cached latch");
        assert!(events.try_recv().is_err());
        let log = std::fs::read_to_string(env.store.log_path()).unwrap();
        assert!(log.contains(log_line), "{log}");
        assert!(!log.contains("STDERR-MARKER"));
    }
}

#[test]
fn not_logged_in_still_edits_todos_and_notes() {
    let env = Env::new(&["example/widget"]);
    env.script.set_probe(Ok(GhProbe::NotLoggedIn {
        version: "2.62.0".into(),
    }));
    env.github.refresh_all(NOW_MS, &settings(14, true)).unwrap();
    let todo = inbox_item(&env, "仍可编辑", None);
    env.todos.rename_item(&todo, "改了标题").unwrap();
    assert_eq!(env.todos.item(&todo).unwrap().item.title, "改了标题");
    let notes = NoteCommands::open(env.store.clone()).unwrap();
    let note = notes
        .create(&NoteInput {
            title: "便签".into(),
            body: "正文".into(),
            tags: Vec::new(),
            pinned: false,
        })
        .unwrap();
    let saved = notes
        .save(
            &note.id,
            note.revision,
            &NoteInput {
                title: "便签".into(),
                body: "改过".into(),
                tags: Vec::new(),
                pinned: false,
            },
        )
        .unwrap();
    assert_eq!(saved.body, "改过");
}

#[test]
fn missing_fields_do_not_close_and_explicit_closed_can() {
    let env = Env::new(&["example/widget"]);
    env.script.set_fetch(
        "example/widget",
        Ok(fetched("missing-prs.json", "empty.json")),
    );
    let open_without_state = inbox_item(
        &env,
        "缺状态",
        Some(source(
            SourceKind::GithubPr,
            "example/widget",
            22,
            "https://github.com/example/widget/pull/22",
        )),
    );
    let closed = inbox_item(
        &env,
        "已关闭",
        Some(source(
            SourceKind::GithubPr,
            "example/widget",
            23,
            "https://github.com/example/widget/pull/23",
        )),
    );
    let absent = inbox_item(
        &env,
        "不在响应里",
        Some(source(
            SourceKind::GithubIssue,
            "example/widget",
            99,
            "https://github.com/example/widget/issues/99",
        )),
    );
    let report = env.github.refresh_all(NOW_MS, &settings(14, true)).unwrap();
    let RepoResult::Updated { sync, .. } = &report.repos[0].result else {
        panic!("{report:?}");
    };
    assert!(sync.ran);
    assert_eq!(sync.completed, vec![closed.clone()]);
    assert!(sync.failures.is_empty());
    assert!(!item_completed(&env, &open_without_state));
    assert!(item_completed(&env, &closed));
    assert!(!item_completed(&env, &absent));
    let items = &env.github.list(NOW_MS, &settings(14, false)).unwrap()[0].items;
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].number, 21);
    assert!(!items[0].draft);
    assert!(!items[0].stale);
}

#[test]
fn reopen_does_not_reopen_todos_and_disabled_setting_does_not_complete() {
    let env = Env::new(&["example/widget"]);
    env.script.set_fetch("example/widget", Ok(widget()));
    let closed = inbox_item(
        &env,
        "报告",
        Some(source(
            SourceKind::GithubIssue,
            "example/widget",
            2,
            "https://github.com/example/widget/issues/2",
        )),
    );
    let merged = inbox_item(
        &env,
        "合并",
        Some(source(
            SourceKind::GithubPr,
            "example/widget",
            3,
            "https://github.com/example/widget/pull/3",
        )),
    );
    let already = inbox_item(
        &env,
        "已完成的打开项",
        Some(source(
            SourceKind::GithubIssue,
            "example/widget",
            8,
            "https://github.com/example/widget/issues/8",
        )),
    );
    env.todos.complete_item(&already).unwrap();
    let held = env
        .github
        .refresh_all(NOW_MS, &settings(14, false))
        .unwrap();
    let RepoResult::Updated { sync, .. } = &held.repos[0].result else {
        panic!("{held:?}");
    };
    assert!(!sync.ran);
    assert!(!item_completed(&env, &closed));
    assert!(!item_completed(&env, &merged));
    let done = env.github.refresh_all(NOW_MS, &settings(14, true)).unwrap();
    let RepoResult::Updated { sync, .. } = &done.repos[0].result else {
        panic!("{done:?}");
    };
    assert!(sync.ran);
    assert!(sync.completed.contains(&closed));
    assert!(sync.completed.contains(&merged));
    assert!(item_completed(&env, &already));
    let mut reopened = widget();
    for item in &mut reopened.issues {
        if item.number == 2 {
            item.state = Some(lanwork_core::github::RemoteState::Open);
        }
    }
    for item in &mut reopened.pulls {
        if item.number == 3 {
            item.state = Some(lanwork_core::github::RemoteState::Open);
            item.title = Some("Landed again".into());
            item.url = Some("https://github.com/example/widget/pull/3".into());
        }
    }
    env.script.set_fetch("example/widget", Ok(reopened));
    env.github
        .refresh_all(NOW_MS + 1, &settings(14, true))
        .unwrap();
    assert!(item_completed(&env, &closed));
    assert!(item_completed(&env, &merged));
    assert!(item_completed(&env, &already));
}

#[test]
fn auto_complete_failure_keeps_that_todo_and_reports() {
    let env = Env::new(&["example/widget"]);
    env.script.set_fetch("example/widget", Ok(widget()));
    let inbox = env.todos.ensure_inbox().unwrap();
    let blocked = env
        .todos
        .create_item(
            &inbox,
            NewTodo {
                title: "月底周期".into(),
                due: CivilDate::try_from_ymd(2026, 1, 30),
                remind_at: None,
                recurrence: Some(Recurrence {
                    rule: RecurrenceRule::Monthly,
                    until: None,
                    month_day: None,
                }),
                source: Some(source(
                    SourceKind::GithubPr,
                    "example/widget",
                    3,
                    "https://github.com/example/widget/pull/3",
                )),
            },
        )
        .unwrap();
    let plain = inbox_item(
        &env,
        "普通",
        Some(source(
            SourceKind::GithubPr,
            "example/widget",
            3,
            "https://github.com/example/widget/pull/3",
        )),
    );
    let report = env.github.refresh_all(NOW_MS, &settings(14, true)).unwrap();
    let RepoResult::Updated { sync, fetched_at } = &report.repos[0].result else {
        panic!("{report:?}");
    };
    assert_eq!(*fetched_at, NOW_MS);
    assert!(
        sync.failures
            .iter()
            .any(|failure| failure.item_id == blocked)
    );
    assert!(sync.completed.contains(&plain));
    assert!(!item_completed(&env, &blocked));
    assert!(item_completed(&env, &plain));
    assert_eq!(
        env.github
            .snapshot("example/widget")
            .unwrap()
            .unwrap()
            .fetched_at,
        NOW_MS
    );
}

#[test]
fn convert_failure_and_snapshot_write_failure_leave_no_partial_todo() {
    let env = Env::new(&["example/widget"]);
    env.script.set_fetch("example/widget", Ok(widget()));
    env.github
        .refresh_all(NOW_MS, &settings(14, false))
        .unwrap();
    let err = env
        .github
        .convert_to_todo("example/widget", SourceKind::GithubPr, 404)
        .unwrap_err();
    assert!(matches!(err, GithubError::ItemNotFound { number: 404, .. }));
    assert!(
        env.todos
            .lists()
            .unwrap()
            .iter()
            .all(|list| list.items.is_empty())
    );

    let inbox = env.todos.ensure_inbox().unwrap();
    let path = env.store.document_path(&DocumentId::Todo(inbox)).unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(
        env.github
            .convert_to_todo("example/widget", SourceKind::GithubPr, 12)
            .is_err()
    );
    assert!(
        env.todos
            .lists()
            .unwrap()
            .iter()
            .all(|list| list.items.is_empty())
    );

    let env = Env::new(&["example/widget"]);
    env.script.set_fetch("example/widget", Ok(widget()));
    env.github
        .refresh_all(NOW_MS, &settings(14, false))
        .unwrap();
    let todo = inbox_item(
        &env,
        "不要误完成",
        Some(source(
            SourceKind::GithubIssue,
            "example/widget",
            2,
            "https://github.com/example/widget/issues/2",
        )),
    );
    let cache_path = env
        .store
        .document_path(&cache_doc("example/widget"))
        .unwrap();
    assert!(cache_path.is_file());
    std::fs::remove_file(&cache_path).unwrap();
    std::fs::create_dir(&cache_path).unwrap();
    let report = env
        .github
        .refresh_all(NOW_MS + 3, &settings(14, true))
        .unwrap();
    assert!(matches!(
        report.repos[0].result,
        RepoResult::Unchanged {
            failure: FetchFailure::Storage(_)
        }
    ));
    assert!(!item_completed(&env, &todo));
    assert_eq!(
        env.github
            .snapshot("example/widget")
            .unwrap()
            .unwrap()
            .fetched_at,
        NOW_MS
    );
    assert!(cache_path.is_dir());
}

#[test]
fn ignore_and_pin_are_remembered() {
    let env = Env::new(&["example/widget"]);
    env.script.set_fetch("example/widget", Ok(widget()));
    env.github
        .refresh_all(NOW_MS, &settings(14, false))
        .unwrap();
    let events = env.store.subscribe();
    assert!(
        env.github
            .set_ignored("", SourceKind::GithubPr, 1, true)
            .is_err()
    );
    assert!(
        env.github
            .set_pinned("example/widget", SourceKind::GithubPr, 0, true)
            .is_err()
    );

    env.github
        .set_ignored("example/widget", SourceKind::GithubPr, 12, true)
        .unwrap();
    env.github
        .set_pinned("example/widget", SourceKind::GithubIssue, 8, true)
        .unwrap();
    let event = events.recv().unwrap();
    assert_eq!(event.kind, EntityKind::GithubWatchlist);
    assert_eq!(event.id, "watchlist");
    assert!(event.revision.is_none());
    let list = env.github.list(NOW_MS, &settings(14, false)).unwrap();
    let pull = list[0].items.iter().find(|item| item.number == 12).unwrap();
    assert!(pull.ignored);
    assert!(!pull.pinned);
    let issue = list[0].items.iter().find(|item| item.number == 8).unwrap();
    assert!(issue.pinned);
    assert!(titles(&env, "example/widget").contains(&"Fix the latch".to_owned()));

    let again = GithubCommands::open(env.store.clone(), env.script.clone(), env.todos.clone());
    again.load().unwrap();
    let watch = again.watchlist().unwrap();
    assert_eq!(watch.ignored.len(), 1);
    assert_eq!(watch.pinned.len(), 1);
    assert_eq!(watch.ignored[0].kind, SourceKind::GithubPr);
}

#[test]
fn add_tracked_checks_owner_repo_and_remove_drops_snapshot_and_source() {
    let env = Env::new(&["example/widget"]);
    env.script.set_fetch("example/widget", Ok(widget()));
    env.github
        .refresh_all(NOW_MS, &settings(14, false))
        .unwrap();
    env.github
        .set_pinned("example/widget", SourceKind::GithubPr, 12, true)
        .unwrap();
    let before = env.github.tracked().unwrap();
    for bad in [
        "example",
        "owner/repo/extra",
        " owner/repo",
        "-owner/repo",
        "owner/",
        "/repo",
    ] {
        let err = env.github.add_tracked(bad).unwrap_err();
        assert!(
            err.to_string().contains("正确格式是 owner/repo"),
            "{bad}: {err}"
        );
    }
    assert_eq!(env.github.tracked().unwrap(), before);

    env.github.add_tracked("example/new").unwrap();
    env.github.add_tracked("example/new").unwrap();
    assert_eq!(
        env.github
            .tracked()
            .unwrap()
            .iter()
            .filter(|repo| repo.as_str() == "example/new")
            .count(),
        1
    );
    assert!(matches!(
        env.github.remove_tracked("example/missing").unwrap_err(),
        GithubError::RepoNotTracked { .. }
    ));

    let open = inbox_item(
        &env,
        "仍打开",
        Some(source(
            SourceKind::GithubPr,
            "example/widget",
            12,
            "https://github.com/example/widget/pull/12",
        )),
    );
    let done = inbox_item(
        &env,
        "已完成",
        Some(source(
            SourceKind::GithubIssue,
            "example/widget",
            8,
            "https://github.com/example/widget/issues/8",
        )),
    );
    env.todos.complete_item(&done).unwrap();
    let trashed = inbox_item(
        &env,
        "在回收站",
        Some(source(
            SourceKind::GithubIssue,
            "example/widget",
            2,
            "https://github.com/example/widget/issues/2",
        )),
    );
    env.todos.soft_delete(&trashed).unwrap();
    let other = inbox_item(
        &env,
        "另一个仓库",
        Some(source(
            SourceKind::GithubPr,
            "example/new",
            1,
            "https://github.com/example/new/pull/1",
        )),
    );

    env.github.remove_tracked("example/widget").unwrap();
    assert!(env.github.snapshot("example/widget").unwrap().is_none());
    assert!(
        !env.store
            .document_path(&cache_doc("example/widget"))
            .unwrap()
            .exists()
    );
    assert!(
        !env.github
            .tracked()
            .unwrap()
            .iter()
            .any(|repo| repo == "example/widget")
    );
    for id in [&open, &done, &trashed] {
        assert!(env.todos.item(id).unwrap().item.source.is_none());
    }
    assert_eq!(
        env.todos.item(&other).unwrap().item.source.unwrap().repo,
        "example/new"
    );
    let watch = env.github.watchlist().unwrap();
    assert!(
        watch
            .pinned
            .iter()
            .any(|mark| mark.repo == "example/widget")
    );
    assert_eq!(env.todos.item(&open).unwrap().item.title, "仍打开");
}

#[test]
fn filters_are_pending_and_disk_cache_without_refresh_is_offline() {
    let env = Env::new(&[]);
    let raw = r#"{"repos":["example/widget"]}"#;
    std::fs::write(
        env.store
            .document_path(&DocumentId::GithubWatchlist)
            .unwrap(),
        raw,
    )
    .unwrap();
    std::fs::write(
        env.store.document_path(&cache_doc("example/widget")).unwrap(),
        r#"{"schemaVersion":1,"repo":"example/widget","fetchedAt":10,"items":[{"kind":"github-pr","number":4,"title":"Old","url":"https://example.com/4"}]}"#,
    )
    .unwrap();
    env.github.load().unwrap();
    let row = &env.github.list(NOW_MS, &settings(14, false)).unwrap()[0];
    assert!(row.offline_cache);
    assert_eq!(row.cache_label, Some("离线缓存"));
    assert_eq!(row.fetched_at, Some(10));
    assert!(!row.items[0].draft);
    assert!(!row.items[0].stale);
    let err = env
        .github
        .list_filtered(
            &GithubFilter {
                draft: true,
                ..GithubFilter::default()
            },
            NOW_MS,
            &settings(14, false),
        )
        .unwrap_err();
    assert_eq!(err.pending_topic(), Some(PendingTopic::SignalFilters));
    assert_eq!(titles(&env, "example/widget"), vec!["Old".to_owned()]);
}

#[test]
fn refresh_publishes_after_commit_and_parse_failure_does_not() {
    let env = Env::new(&["example/widget"]);
    env.script.set_fetch("example/widget", Ok(widget()));
    let events = env.store.subscribe();
    env.github
        .refresh_all(NOW_MS, &settings(14, false))
        .unwrap();
    let event = events.recv().unwrap();
    assert_eq!(event.kind, EntityKind::GithubCache);
    assert_eq!(event.id, "example%2Fwidget");
    assert!(event.revision.is_none());
    env.script
        .set_fetch("example/widget", Err(GhCallError::Parse));
    env.github
        .refresh_all(NOW_MS + 1, &settings(14, true))
        .unwrap();
    assert!(events.try_recv().is_err());
    assert_eq!(
        env.github
            .snapshot("example/widget")
            .unwrap()
            .unwrap()
            .fetched_at,
        NOW_MS
    );
}

#[test]
fn unsupported_watchlist_schema_does_not_block_todos() {
    let temp = TempDir::new();
    let store = Store::open(StorePaths {
        data_dir: temp.path.join("data"),
        cache_dir: temp.path.join("cache"),
        user_profile: temp.path.join("profile"),
        local_app_data: temp.path.join("local"),
    })
    .unwrap();
    let todos = TodoCommands::open(store.clone());
    todos.boot().unwrap();
    std::fs::write(
        store.document_path(&DocumentId::GithubWatchlist).unwrap(),
        r#"{"schemaVersion":99,"repos":["example/widget"]}"#,
    )
    .unwrap();
    let github = GithubCommands::open(store, ScriptGh::new(), todos.clone());
    assert!(matches!(
        github.load().unwrap_err(),
        GithubError::UnsupportedSchema { found: 99 }
    ));
    assert!(matches!(
        github.list(NOW_MS, &settings(14, false)).unwrap_err(),
        GithubError::NotReady
    ));
    assert!(todos.create_list("工作").is_ok());
}

#[test]
fn process_client_logs_version_only() {
    let pulls = String::from_utf8(fixture("widget-prs.json")).unwrap();
    let issues = String::from_utf8(fixture("widget-issues.json")).unwrap();
    let logged_out = Arc::new(ScriptRunner {
        calls: Mutex::new(Vec::new()),
        responses: vec![
            Ok(capture(
                0,
                "gh version 2.62.0 (2024-10-21)\n",
                "ghp_abcdefghijklmnopqrst STDERR-MARKER",
            )),
            Ok(capture(
                1,
                "",
                "You are not logged into any GitHub hosts STDERR-MARKER",
            )),
        ],
    });
    let env = Env::new(&["example/widget"]);
    let github = GithubCommands::open(
        env.store.clone(),
        Arc::new(ProcessGh::with_runner(logged_out.clone())),
        env.todos.clone(),
    );
    github.load().unwrap();
    let probe = github.probe().unwrap();
    assert_eq!(
        probe,
        GhProbe::NotLoggedIn {
            version: "2.62.0".into()
        }
    );
    assert_eq!(logged_out.calls.lock().unwrap().len(), 2);
    let runner = Arc::new(ScriptRunner {
        calls: Mutex::new(Vec::new()),
        responses: vec![
            Ok(capture(
                0,
                "gh version 2.62.0 (2024-10-21)\n",
                "STDERR-MARKER",
            )),
            Ok(capture(0, "", "GH_TOKEN=should-not-be-logged")),
            Ok(capture(0, &pulls, "STDERR-MARKER")),
            Ok(capture(0, &issues, "")),
        ],
    });
    let github = GithubCommands::open(
        env.store.clone(),
        Arc::new(ProcessGh::with_runner(runner.clone())),
        env.todos.clone(),
    );
    github.load().unwrap();
    github.refresh_all(NOW_MS, &settings(14, false)).unwrap();
    assert_eq!(titles_of(&github, "example/widget").len(), 3);
    let log = std::fs::read_to_string(env.store.log_path()).unwrap();
    assert!(log.contains("gh version 2.62.0"), "{log}");
    assert!(!log.contains("STDERR-MARKER"), "{log}");
    assert!(!log.contains("ghp_"), "{log}");
    assert!(!log.contains("GH_TOKEN"), "{log}");
    assert!(!log.contains("should-not-be-logged"), "{log}");
    assert_no_secrets(&env.temp.path.join("data"));
}

fn titles_of(github: &GithubCommands, repo: &str) -> Vec<String> {
    github
        .list(NOW_MS, &settings(14, false))
        .unwrap()
        .into_iter()
        .find(|list| list.repo == repo)
        .unwrap()
        .items
        .into_iter()
        .map(|item| item.title)
        .collect()
}
