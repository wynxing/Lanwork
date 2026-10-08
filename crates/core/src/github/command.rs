//! GitHub 薄命令。
//!
//! 界面只调用这里。添加和移除追踪仓库在 #9 第 17 项写入产品规格之前不落盘。
//! 任一筛选项被选中时，在 #9 第 18 项写入产品规格之前不筛选。
//! 打开条目只返回 `http` / `https` URL，由外壳用系统浏览器打开。

use std::sync::Arc;

use crate::storage::Store;
use crate::todos::{SourceKind, TodoCommands};

use super::error::{GhCallError, GithubError, PendingTopic};
use super::gh::{GhClient, GhProbe};
use super::model::{GithubFilter, GithubSettings, RepoList, RepoSnapshot, Watchlist};
use super::service::{RefreshReport, Service};
use super::signals::SignalExtension;

/// 界面使用的 GitHub 命令。克隆后仍是同一份数据和同一把操作锁。
#[derive(Clone)]
pub struct GithubCommands {
    service: Arc<Service>,
}

impl std::fmt::Debug for GithubCommands {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GithubCommands").finish_non_exhaustive()
    }
}

impl GithubCommands {
    #[must_use]
    pub fn open(store: Store, gh: Arc<dyn GhClient>, todos: TodoCommands) -> Self {
        Self {
            service: Service::open(store, gh, todos),
        }
    }

    #[must_use]
    pub fn store(&self) -> Store {
        self.service.store()
    }

    /// 读 watchlist 和已有快照。存储须已经启动。文件不存在时 watchlist 为空，不写盘。
    pub fn load(&self) -> Result<(), GithubError> {
        self.service.load()
    }

    /// 第 18 项的扩展点。传入 `None` 就去掉。服务不根据它改变列表。
    pub fn set_signal_extension(&self, extension: Option<Arc<dyn SignalExtension>>) {
        self.service.set_signal_extension(extension);
    }

    pub fn probe(&self) -> Result<GhProbe, GhCallError> {
        self.service.probe()
    }

    pub fn watchlist(&self) -> Result<Watchlist, GithubError> {
        self.service.watchlist()
    }

    pub fn tracked(&self) -> Result<Vec<String>, GithubError> {
        Ok(self.service.watchlist()?.repos)
    }

    /// #9 第 17 项。不校验输入，不写 watchlist，不改快照和待办。
    pub fn add_tracked(&self, name: &str) -> Result<(), GithubError> {
        let _ = name;
        Err(GithubError::PendingSpec(PendingTopic::RepoManagement))
    }

    /// #9 第 17 项。不移除缓存，也不改关联待办。
    pub fn remove_tracked(&self, name: &str) -> Result<(), GithubError> {
        let _ = name;
        Err(GithubError::PendingSpec(PendingTopic::RepoManagement))
    }

    pub fn set_ignored(
        &self,
        repo: &str,
        kind: SourceKind,
        number: u64,
        ignored: bool,
    ) -> Result<(), GithubError> {
        self.service.set_ignored(repo, kind, number, ignored)
    }

    pub fn set_pinned(
        &self,
        repo: &str,
        kind: SourceKind,
        number: u64,
        pinned: bool,
    ) -> Result<(), GithubError> {
        self.service.set_pinned(repo, kind, number, pinned)
    }

    pub fn refresh_all(
        &self,
        now_ms: i64,
        settings: &GithubSettings,
    ) -> Result<RefreshReport, GithubError> {
        self.service.refresh_all(now_ms, settings)
    }

    pub fn refresh_repo(
        &self,
        repo: &str,
        now_ms: i64,
        settings: &GithubSettings,
    ) -> Result<RefreshReport, GithubError> {
        self.service.refresh_repo(repo, now_ms, settings)
    }

    /// 无筛选列表。Draft 和长期未更新只作为每条上的状态，不在这里过滤。
    pub fn list(
        &self,
        now_ms: i64,
        settings: &GithubSettings,
    ) -> Result<Vec<RepoList>, GithubError> {
        self.service.lists(now_ms, settings.stale_days)
    }

    /// 没有筛选项时与 [`Self::list`] 相同。有任一筛选项则返回未定错误，不改数据。
    pub fn list_filtered(
        &self,
        filter: &GithubFilter,
        now_ms: i64,
        settings: &GithubSettings,
    ) -> Result<Vec<RepoList>, GithubError> {
        if !filter.is_unfiltered() {
            return Err(GithubError::PendingSpec(PendingTopic::SignalFilters));
        }
        self.list(now_ms, settings)
    }

    /// 一次写入收件箱条目和来源。失败时不留下这条待办。
    pub fn convert_to_todo(
        &self,
        repo: &str,
        kind: SourceKind,
        number: u64,
    ) -> Result<String, GithubError> {
        self.service.convert(repo, kind, number)
    }

    pub fn open_url(
        &self,
        repo: &str,
        kind: SourceKind,
        number: u64,
    ) -> Result<String, GithubError> {
        self.service.open_url(repo, kind, number)
    }

    pub fn snapshot(&self, repo: &str) -> Result<Option<RepoSnapshot>, GithubError> {
        self.service.snapshot(repo)
    }
}
