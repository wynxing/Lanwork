//! GitHub 规则和写盘。
//!
//! 内存只在对应写入成功后换成新状态。快照和 watchlist 各是一个文件，写完才发布
//! `EntityChanged`。某个仓库失败时不写它的文件，也不改其他仓库的快照。
//! 来源同步发生在该仓库快照提交之后，失败不回滚快照。
//!
//! 变更回调里可以读列表，不要再调用刷新、转为待办或忽略、钉住，否则会和操作锁死锁。
//! 回调当时看到的仍是上一次成功写入后的内存。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::storage::{
    CollectionKind, DocumentId, SCHEMA_VERSION, Store, is_supported_schema, lock_mutex,
};
use crate::todos::{NewTodo, SourceKind, TodoCommands, TodoSource, is_http_source_url};

use super::error::{FetchFailure, GhCallError, GithubError};
use super::gh::{GhClient, GhProbe, version_log_line};
use super::model::{
    ItemMark, ListedItem, OFFLINE_CACHE_LABEL, RepoList, RepoSnapshot, SnapshotItem, Watchlist,
    cache_file_id, check_mark, mark_is_set, set_mark, todo_title, unique_repos,
};
use super::parse::{FetchedRepo, ParsedItem, RemoteState};
use super::signals::{SignalExtension, is_stale};

/// 一条自动完成没有写成的待办。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncFailure {
    pub item_id: String,
    pub message: String,
}

/// 来源同步的结果。`ran` 为假表示设置不允许，没有读待办去改完成状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub ran: bool,
    pub completed: Vec<String>,
    pub failures: Vec<SyncFailure>,
}

impl SyncReport {
    fn skipped() -> Self {
        Self {
            ran: false,
            completed: Vec::new(),
            failures: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoResult {
    Updated { fetched_at: i64, sync: SyncReport },
    Unchanged { failure: FetchFailure },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoOutcome {
    pub repo: String,
    pub result: RepoResult,
}

/// 一次刷新。单个仓库失败记在对应的 [`RepoOutcome`] 里，不让其他仓库作废。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshReport {
    pub refresh_interval_ms: u64,
    pub source_sync: bool,
    pub repos: Vec<RepoOutcome>,
}

#[derive(Clone, Default)]
struct RepoState {
    snapshot: Option<RepoSnapshot>,
    offline_cache: bool,
    failure: Option<FetchFailure>,
}

#[derive(Clone)]
struct State {
    loaded: bool,
    watchlist: Watchlist,
    repos: BTreeMap<String, RepoState>,
}

impl State {
    fn empty() -> Self {
        Self {
            loaded: false,
            watchlist: Watchlist::empty(),
            repos: BTreeMap::new(),
        }
    }
}

impl crate::storage::MemoryReload for Service {
    fn reload_memory(&self) -> Result<(), String> {
        let loaded = lock_mutex(&self.state).loaded;
        if !loaded {
            return Ok(());
        }
        self.load().map_err(|err| err.to_string())
    }
}

pub(crate) struct Service {
    store: Store,
    gh: Arc<dyn GhClient>,
    todos: TodoCommands,
    op: Mutex<()>,
    state: Mutex<State>,
    extension: Mutex<Option<Arc<dyn SignalExtension>>>,
}

impl Service {
    pub(crate) fn open(store: Store, gh: Arc<dyn GhClient>, todos: TodoCommands) -> Arc<Self> {
        let service = Arc::new(Self {
            store,
            gh,
            todos,
            op: Mutex::new(()),
            state: Mutex::new(State::empty()),
            extension: Mutex::new(None),
        });
        let reload: Arc<dyn crate::storage::MemoryReload> = service.clone();
        service.store.watch_memory(&reload);
        drop(reload);
        service
    }

    pub(crate) fn store(&self) -> Store {
        self.store.clone()
    }

    pub(crate) fn set_signal_extension(&self, extension: Option<Arc<dyn SignalExtension>>) {
        *lock_mutex(&self.extension) = extension;
    }

    /// 第一次成功之前，失败会让命令保持未加载。
    /// 已经载入之后再次失败，内存仍留着上一次成功的内容。
    pub(crate) fn load(&self) -> Result<(), GithubError> {
        let _op = lock_mutex(&self.op);
        if !self.store.is_ready() {
            return Err(GithubError::NotReady);
        }
        let watchlist = match self
            .store
            .read_json::<Watchlist>(&DocumentId::GithubWatchlist)?
        {
            None => Watchlist::empty(),
            Some(list) => {
                if !is_supported_schema(list.schema_version) {
                    return Err(GithubError::UnsupportedSchema {
                        found: list.schema_version,
                    });
                }
                list
            }
        };
        let loaded = self
            .store
            .read_collection::<RepoSnapshot>(CollectionKind::GithubCaches)?;
        let mut repos = BTreeMap::new();
        for file in loaded.files {
            if !is_supported_schema(file.value.schema_version) || file.value.repo.is_empty() {
                continue;
            }
            repos.insert(
                file.value.repo.clone(),
                RepoState {
                    snapshot: Some(file.value),
                    offline_cache: true,
                    failure: None,
                },
            );
        }
        *lock_mutex(&self.state) = State {
            loaded: true,
            watchlist,
            repos,
        };
        Ok(())
    }

    pub(crate) fn probe(&self) -> Result<GhProbe, GhCallError> {
        let probe = self.gh.probe();
        self.log_probe(&probe);
        probe
    }

    pub(crate) fn watchlist(&self) -> Result<Watchlist, GithubError> {
        let state = lock_mutex(&self.state);
        if !state.loaded {
            return Err(GithubError::NotReady);
        }
        Ok(state.watchlist.clone())
    }

    pub(crate) fn snapshot(&self, repo: &str) -> Result<Option<RepoSnapshot>, GithubError> {
        let state = lock_mutex(&self.state);
        if !state.loaded {
            return Err(GithubError::NotReady);
        }
        Ok(state
            .repos
            .get(repo)
            .and_then(|entry| entry.snapshot.clone()))
    }

    pub(crate) fn set_ignored(
        &self,
        repo: &str,
        kind: SourceKind,
        number: u64,
        on: bool,
    ) -> Result<(), GithubError> {
        self.set_flag(repo, kind, number, on, true)
    }

    pub(crate) fn set_pinned(
        &self,
        repo: &str,
        kind: SourceKind,
        number: u64,
        on: bool,
    ) -> Result<(), GithubError> {
        self.set_flag(repo, kind, number, on, false)
    }

    pub(crate) fn refresh_all(
        &self,
        now_ms: i64,
        settings: &super::model::GithubSettings,
    ) -> Result<RefreshReport, GithubError> {
        let _op = lock_mutex(&self.op);
        self.ensure_loaded()?;
        let repos = unique_repos(&lock_mutex(&self.state).watchlist.repos);
        self.refresh_repos(&repos, now_ms, settings)
    }

    pub(crate) fn refresh_repo(
        &self,
        repo: &str,
        now_ms: i64,
        settings: &super::model::GithubSettings,
    ) -> Result<RefreshReport, GithubError> {
        let _op = lock_mutex(&self.op);
        self.ensure_loaded()?;
        let tracked = lock_mutex(&self.state)
            .watchlist
            .repos
            .iter()
            .any(|item| item == repo);
        if !tracked {
            return Err(GithubError::RepoNotTracked {
                repo: repo.to_owned(),
            });
        }
        self.refresh_repos(&[repo.to_owned()], now_ms, settings)
    }

    /// 无筛选的列表。存在未完成且未软删除的关联待办时，该条不在结果里。
    ///
    /// 「命中当前筛选」等 #9 第 18 项。这里没有筛选。
    pub(crate) fn lists(&self, now_ms: i64, stale_days: u32) -> Result<Vec<RepoList>, GithubError> {
        let state = {
            let state = lock_mutex(&self.state);
            if !state.loaded {
                return Err(GithubError::NotReady);
            }
            state.clone()
        };
        let hiding = self.hiding_keys()?;
        let extension = lock_mutex(&self.extension).clone();
        let mut lists = Vec::new();
        for repo in unique_repos(&state.watchlist.repos) {
            let Some(entry) = state.repos.get(&repo) else {
                lists.push(empty_repo(repo));
                continue;
            };
            let mut items = Vec::new();
            if let Some(snapshot) = &entry.snapshot {
                for item in &snapshot.items {
                    if hiding.iter().any(|(kind, name, number)| {
                        *kind == item.kind && name == &snapshot.repo && *number == item.number
                    }) {
                        continue;
                    }
                    if let Some(extension) = &extension {
                        extension.observe(item);
                    }
                    items.push(ListedItem {
                        repo: snapshot.repo.clone(),
                        kind: item.kind,
                        number: item.number,
                        title: item.title.clone(),
                        url: item.url.clone(),
                        draft: item.draft,
                        stale: is_stale(item.updated_at, now_ms, stale_days),
                        ignored: mark_is_set(
                            &state.watchlist.ignored,
                            &snapshot.repo,
                            item.kind,
                            item.number,
                        ),
                        pinned: mark_is_set(
                            &state.watchlist.pinned,
                            &snapshot.repo,
                            item.kind,
                            item.number,
                        ),
                    });
                }
            }
            lists.push(RepoList {
                repo,
                fetched_at: entry.snapshot.as_ref().map(|snapshot| snapshot.fetched_at),
                offline_cache: entry.offline_cache,
                cache_label: entry.offline_cache.then_some(OFFLINE_CACHE_LABEL),
                failure: entry.failure.clone(),
                items,
            });
        }
        Ok(lists)
    }

    pub(crate) fn convert(
        &self,
        repo: &str,
        kind: SourceKind,
        number: u64,
    ) -> Result<String, GithubError> {
        let _op = lock_mutex(&self.op);
        self.ensure_loaded()?;
        let item = {
            let state = lock_mutex(&self.state);
            state
                .repos
                .get(repo)
                .and_then(|entry| entry.snapshot.as_ref())
                .and_then(|snapshot| {
                    snapshot
                        .items
                        .iter()
                        .find(|item| item.kind == kind && item.number == number)
                        .cloned()
                })
        };
        let Some(item) = item else {
            return Err(GithubError::ItemNotFound {
                repo: repo.to_owned(),
                kind,
                number,
            });
        };
        let source = TodoSource::try_new(kind, item.url, repo, number)?;
        let inbox = self.todos.ensure_inbox()?;
        self.todos
            .create_item(
                &inbox,
                NewTodo {
                    title: todo_title(repo, number, &item.title),
                    due: None,
                    remind_at: None,
                    recurrence: None,
                    source: Some(source),
                },
            )
            .map_err(GithubError::from)
    }

    pub(crate) fn open_url(
        &self,
        repo: &str,
        kind: SourceKind,
        number: u64,
    ) -> Result<String, GithubError> {
        let state = lock_mutex(&self.state);
        if !state.loaded {
            return Err(GithubError::NotReady);
        }
        let url = state
            .repos
            .get(repo)
            .and_then(|entry| entry.snapshot.as_ref())
            .and_then(|snapshot| {
                snapshot
                    .items
                    .iter()
                    .find(|item| item.kind == kind && item.number == number)
                    .map(|item| item.url.clone())
            });
        let Some(url) = url else {
            return Err(GithubError::ItemNotFound {
                repo: repo.to_owned(),
                kind,
                number,
            });
        };
        if !is_http_source_url(&url) {
            return Err(GithubError::InvalidItem);
        }
        Ok(url)
    }

    fn set_flag(
        &self,
        repo: &str,
        kind: SourceKind,
        number: u64,
        on: bool,
        ignored: bool,
    ) -> Result<(), GithubError> {
        check_mark(repo, number)?;
        let _op = lock_mutex(&self.op);
        self.ensure_loaded()?;
        let mut list = lock_mutex(&self.state).watchlist.clone();
        let marks = if ignored {
            &mut list.ignored
        } else {
            &mut list.pinned
        };
        if !set_mark(
            marks,
            ItemMark {
                repo: repo.to_owned(),
                kind,
                number,
            },
            on,
        ) {
            return Ok(());
        }
        self.store.write_json(&DocumentId::GithubWatchlist, &list)?;
        lock_mutex(&self.state).watchlist = list;
        Ok(())
    }

    fn refresh_repos(
        &self,
        repos: &[String],
        now_ms: i64,
        settings: &super::model::GithubSettings,
    ) -> Result<RefreshReport, GithubError> {
        let probe = self.gh.probe();
        self.log_probe(&probe);
        let mut outcomes = Vec::with_capacity(repos.len());
        match probe {
            Ok(GhProbe::Ready { .. }) => {
                for repo in repos {
                    outcomes.push(self.refresh_one(repo, now_ms, settings));
                }
            }
            Ok(GhProbe::NotInstalled) => {
                for repo in repos {
                    outcomes.push(self.keep(repo, FetchFailure::NotInstalled));
                }
            }
            Ok(GhProbe::NotLoggedIn { .. }) => {
                for repo in repos {
                    outcomes.push(self.keep(repo, FetchFailure::NotLoggedIn));
                }
            }
            Err(err) => {
                let failure = FetchFailure::from_call(err);
                for repo in repos {
                    outcomes.push(self.keep(repo, failure.clone()));
                }
            }
        }
        Ok(RefreshReport {
            refresh_interval_ms: settings.refresh_interval_ms,
            source_sync: settings.source_sync,
            repos: outcomes,
        })
    }

    fn refresh_one(
        &self,
        repo: &str,
        now_ms: i64,
        settings: &super::model::GithubSettings,
    ) -> RepoOutcome {
        if cache_file_id(repo).is_err() {
            return self.keep(repo, FetchFailure::InvalidRepo);
        }
        match self.gh.fetch(repo) {
            Ok(fetched) => self.apply_fetch(repo, now_ms, settings, &fetched),
            Err(err) => self.keep(repo, FetchFailure::from_call(err)),
        }
    }

    fn apply_fetch(
        &self,
        repo: &str,
        now_ms: i64,
        settings: &super::model::GithubSettings,
        fetched: &FetchedRepo,
    ) -> RepoOutcome {
        let mut items = Vec::new();
        for pull in &fetched.pulls {
            if let Some(item) = to_open_item(SourceKind::GithubPr, pull) {
                items.push(item);
            }
        }
        for issue in &fetched.issues {
            if let Some(item) = to_open_item(SourceKind::GithubIssue, issue) {
                items.push(item);
            }
        }
        let snapshot = RepoSnapshot {
            schema_version: SCHEMA_VERSION,
            repo: repo.to_owned(),
            fetched_at: now_ms,
            items,
        };
        let Ok(id) = cache_file_id(repo) else {
            return self.keep(repo, FetchFailure::InvalidRepo);
        };
        if let Err(err) = self
            .store
            .write_json(&DocumentId::GithubCache(id), &snapshot)
        {
            return self.keep(repo, FetchFailure::Storage(err.to_string()));
        }
        self.remember(snapshot);
        let sync = self.sync_repo(repo, fetched, settings);
        RepoOutcome {
            repo: repo.to_owned(),
            result: RepoResult::Updated {
                fetched_at: now_ms,
                sync,
            },
        }
    }

    fn keep(&self, repo: &str, failure: FetchFailure) -> RepoOutcome {
        let mut state = lock_mutex(&self.state);
        let entry = state.repos.entry(repo.to_owned()).or_default();
        entry.failure = Some(failure.clone());
        if entry.snapshot.is_some() {
            entry.offline_cache = true;
        }
        RepoOutcome {
            repo: repo.to_owned(),
            result: RepoResult::Unchanged { failure },
        }
    }

    fn remember(&self, snapshot: RepoSnapshot) {
        let mut state = lock_mutex(&self.state);
        state.repos.insert(
            snapshot.repo.clone(),
            RepoState {
                snapshot: Some(snapshot),
                offline_cache: false,
                failure: None,
            },
        );
    }

    fn sync_repo(
        &self,
        repo: &str,
        fetched: &FetchedRepo,
        settings: &super::model::GithubSettings,
    ) -> SyncReport {
        if !settings.auto_complete_on_close {
            return SyncReport::skipped();
        }
        let mut states = Vec::new();
        push_states(&mut states, SourceKind::GithubPr, &fetched.pulls);
        push_states(&mut states, SourceKind::GithubIssue, &fetched.issues);
        let lists = match self.todos.lists() {
            Ok(lists) => lists,
            Err(err) => {
                return SyncReport {
                    ran: true,
                    completed: Vec::new(),
                    failures: vec![SyncFailure {
                        item_id: String::new(),
                        message: err.to_string(),
                    }],
                };
            }
        };
        let mut completed = Vec::new();
        let mut failures = Vec::new();
        for list in lists {
            for item in list.items {
                let Some(ref source) = item.source else {
                    continue;
                };
                if source.repo != repo {
                    continue;
                }
                let Some(state) = states.iter().find_map(|(kind, number, state)| {
                    (*kind == source.kind && *number == source.number).then_some(*state)
                }) else {
                    continue;
                };
                if !matches!(state, RemoteState::Closed | RemoteState::Merged) {
                    continue;
                }
                if item.completed || item.in_trash() {
                    continue;
                }
                match self.todos.complete_item(&item.id) {
                    Ok(()) => completed.push(item.id),
                    Err(err) => failures.push(SyncFailure {
                        item_id: item.id,
                        message: err.to_string(),
                    }),
                }
            }
        }
        SyncReport {
            ran: true,
            completed,
            failures,
        }
    }

    fn hiding_keys(&self) -> Result<Vec<(SourceKind, String, u64)>, GithubError> {
        let mut keys = Vec::new();
        for list in self.todos.lists()? {
            for item in list.items {
                if item.completed || item.in_trash() {
                    continue;
                }
                if let Some(source) = item.source {
                    keys.push((source.kind, source.repo, source.number));
                }
            }
        }
        Ok(keys)
    }

    fn ensure_loaded(&self) -> Result<(), GithubError> {
        if lock_mutex(&self.state).loaded {
            Ok(())
        } else {
            Err(GithubError::NotReady)
        }
    }

    fn log_probe(&self, probe: &Result<GhProbe, GhCallError>) {
        match probe {
            Ok(GhProbe::Ready { version } | GhProbe::NotLoggedIn { version }) => {
                self.store.log_info(&version_log_line(version));
            }
            Ok(GhProbe::NotInstalled) => self.store.log_info("gh not installed"),
            Err(_) => self.store.log_info("gh unavailable"),
        }
    }
}

fn empty_repo(repo: String) -> RepoList {
    RepoList {
        repo,
        fetched_at: None,
        offline_cache: false,
        cache_label: None,
        failure: None,
        items: Vec::new(),
    }
}

fn to_open_item(kind: SourceKind, item: &ParsedItem) -> Option<SnapshotItem> {
    if item.state != Some(RemoteState::Open) {
        return None;
    }
    let title = item
        .title
        .as_ref()
        .filter(|title| !title.trim().is_empty())?;
    let url = item.url.as_ref().filter(|url| is_http_source_url(url))?;
    Some(SnapshotItem {
        kind,
        number: item.number,
        title: title.clone(),
        url: url.clone(),
        updated_at: item.updated_at_ms,
        draft: kind == SourceKind::GithubPr && item.draft,
    })
}

fn push_states(
    out: &mut Vec<(SourceKind, u64, RemoteState)>,
    kind: SourceKind,
    items: &[ParsedItem],
) {
    for item in items {
        let Some(state) = item.state else {
            continue;
        };
        if out
            .iter()
            .any(|(existing, number, _)| *existing == kind && *number == item.number)
        {
            continue;
        }
        out.push((kind, item.number, state));
    }
}
