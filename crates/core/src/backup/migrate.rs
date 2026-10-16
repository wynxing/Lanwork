//! 把当前数据目录迁到新位置。
//!
//! 顺序是：暂停写入、复制、逐个校验、写引导文件、再把已打开的存储指到新目录。
//! 引导文件是唯一切换点。写它之前失败或进程结束，下次启动仍用旧目录。
//! 旧目录不删除。导出白名单以外的文件不复制，留在旧目录。
//! `todo-purge-pending.json` 例外：它和清单、收纳分组一起复制，否则下次启动补不上解除。

use std::path::{Path, PathBuf};

use super::package::data_file_names;
use crate::storage::{self, Store, absolute_lexical, is_same_or_under, write_bootstrap};

/// 永久删除尚未让收纳解除时的记录。不在导出白名单里，迁移仍要复制。
const PURGE_PENDING: &str = "todo-purge-pending.json";

/// 迁移进行到哪一步。界面只拿这个显示进度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigratePhase {
    Copy,
    Verify,
    Switch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrateProgress {
    pub phase: MigratePhase,
    pub done: u64,
    pub total: u64,
}

/// 可以显示在设置页的迁移错误。不含文件正文。
#[derive(Debug)]
pub enum MigrateError {
    SameDirectory,
    Nested,
    EnvBlocked,
    AlreadyHasData,
    ImportPending,
    NotADirectory {
        path: PathBuf,
    },
    NoSpace,
    VerifyFailed {
        leftovers: bool,
    },
    NotReady,
    /// 测试用来模拟复制过程中进程结束。不写引导文件，也不删已经复制的文件。
    Stopped,
    Io {
        message: String,
    },
}

impl std::fmt::Display for MigrateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SameDirectory => write!(f, "已经是当前数据目录"),
            Self::Nested => write!(
                f,
                "新目录不能位于当前数据目录之内，当前数据目录也不能位于新目录之内"
            ),
            Self::EnvBlocked => write!(
                f,
                "数据目录由环境变量 LANWORK_DATA_DIR 指定，不能在设置里更换"
            ),
            Self::AlreadyHasData => {
                write!(f, "该目录已有 Lanwork 数据，不会合并，仍使用原来的目录")
            }
            Self::ImportPending => write!(f, "导入尚未完成，不能更换数据目录"),
            Self::NotADirectory { path } => write!(f, "不是文件夹：{}", path.display()),
            Self::NoSpace => write!(f, "空间不足，仍使用原来的目录"),
            Self::VerifyFailed { leftovers } => {
                if *leftovers {
                    write!(f, "校验失败，仍使用原来的目录。新目录里留下了未完成的文件")
                } else {
                    write!(f, "校验失败，仍使用原来的目录")
                }
            }
            Self::NotReady => write!(f, "启动未完成"),
            Self::Stopped => write!(f, "迁移已中断，仍使用原来的目录"),
            Self::Io { message } => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for MigrateError {}

#[must_use]
pub fn progress_text(progress: MigrateProgress) -> String {
    match progress.phase {
        MigratePhase::Switch => "正在切换".to_owned(),
        MigratePhase::Copy => format!("正在复制 {}/{}", progress.done, progress.total),
        MigratePhase::Verify => format!("正在校验 {}/{}", progress.done, progress.total),
    }
}

/// 只在测试里打开。正式迁移这些都是关闭的。
#[derive(Default)]
struct Fault {
    die_after_copies: Option<u64>,
    corrupt_first: bool,
    skip_bootstrap: bool,
    force_no_space: bool,
}

/// 迁到 `target`。`env_blocks` 为真表示 `LANWORK_DATA_DIR` 非空，引导文件改了下次启动也不会用。
///
/// `progress` 在写锁里调用，不能再写入这份存储，也不能再调用本函数。
/// 成功时返回新目录，并且 [`Store::data_dir`] 已经是它。失败时存储仍指向旧目录。
pub fn migrate_data_dir(
    store: &Store,
    target: &Path,
    env_blocks: bool,
    progress: impl FnMut(MigrateProgress),
) -> Result<PathBuf, MigrateError> {
    migrate(store, target, env_blocks, Fault::default(), progress)
}

fn migrate(
    store: &Store,
    target: &Path,
    env_blocks: bool,
    fault: Fault,
    mut progress: impl FnMut(MigrateProgress),
) -> Result<PathBuf, MigrateError> {
    if env_blocks {
        return Err(MigrateError::EnvBlocked);
    }
    store.ensure_can_write().map_err(|err| match err {
        storage::Error::NotReady => MigrateError::NotReady,
        other => io_message(other),
    })?;
    let source = store.data_dir();
    let target = absolute_lexical(target).map_err(io_message)?;
    storage::reject_maydolist(&target, store.user_profile()).map_err(io_message)?;
    if is_same_or_under(&target, &source) && is_same_or_under(&source, &target) {
        return Err(MigrateError::SameDirectory);
    }
    if is_same_or_under(&target, &source) || is_same_or_under(&source, &target) {
        return Err(MigrateError::Nested);
    }
    if storage::fs_exists(&target) && !target.is_dir() {
        return Err(MigrateError::NotADirectory { path: target });
    }
    storage::fs_create_dir_all(&target)
        .map_err(|err| map_io(&target, err, storage::IoAction::CreateDir))?;
    probe_writable(&target)?;

    store.with_write_lock(|| migrate_holding(store, &source, &target, &fault, &mut progress))
}

fn migrate_holding(
    store: &Store,
    source: &Path,
    target: &Path,
    fault: &Fault,
    progress: &mut impl FnMut(MigrateProgress),
) -> Result<PathBuf, MigrateError> {
    if storage::fs_exists(&source.join("import.pending")) {
        return Err(MigrateError::ImportPending);
    }
    let names = migration_names(source)?;
    ensure_space(target, source, &names, fault.force_no_space)?;
    let total = names.len() as u64;
    let preexisting = has_lanwork_data(target)?;
    let mut written = Vec::new();
    if preexisting {
        let target_names = migration_names(target)?;
        if target_names != names || !all_match(source, target, &names)? {
            return Err(MigrateError::AlreadyHasData);
        }
        report(progress, MigratePhase::Copy, total, total);
        report(progress, MigratePhase::Verify, total, total);
    } else {
        report(progress, MigratePhase::Copy, 0, total);
        for (index, name) in names.iter().enumerate() {
            if fault.die_after_copies == Some(index as u64) {
                return Err(MigrateError::Stopped);
            }
            let from = join(source, name);
            let dest = join(target, name);
            let bytes = storage::fs_read(&from)
                .map_err(|err| map_io(&from, err, storage::IoAction::Read))?;
            if let Err(err) = storage::atomic_write(&dest, &bytes) {
                let leftovers = !remove_copied(&written);
                let migrate_err = map_io(&dest, err, storage::IoAction::Write);
                if leftovers {
                    return Err(MigrateError::Io {
                        message: format!("{migrate_err}。新目录里留下了未完成的文件"),
                    });
                }
                return Err(migrate_err);
            }
            written.push(dest);
            report(progress, MigratePhase::Copy, index as u64 + 1, total);
        }
        if fault.corrupt_first
            && let Some(path) = written.first()
        {
            let mut bytes = storage::fs_read(path).unwrap_or_default();
            if bytes.is_empty() {
                bytes.push(0);
            } else {
                bytes[0] ^= 0xff;
            }
            let _ = storage::atomic_write(path, &bytes);
        }
        report(progress, MigratePhase::Verify, 0, total);
        for (index, name) in names.iter().enumerate() {
            if !same_file(&join(source, name), &join(target, name))? {
                let leftovers = !remove_copied(&written);
                return Err(MigrateError::VerifyFailed { leftovers });
            }
            report(progress, MigratePhase::Verify, index as u64 + 1, total);
        }
    }
    if fault.skip_bootstrap {
        return Err(MigrateError::Io {
            message: "引导文件没有写入，仍使用原来的目录".to_owned(),
        });
    }
    report(progress, MigratePhase::Switch, 0, total);
    write_bootstrap(store.local_app_data(), target).map_err(io_message)?;
    store.adopt_data_dir(target.to_path_buf());
    Ok(target.to_path_buf())
}

fn report(progress: &mut impl FnMut(MigrateProgress), phase: MigratePhase, done: u64, total: u64) {
    progress(MigrateProgress { phase, done, total });
}

fn all_match(source: &Path, target: &Path, names: &[String]) -> Result<bool, MigrateError> {
    for name in names {
        if !same_file(&join(source, name), &join(target, name))? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn same_file(left: &Path, right: &Path) -> Result<bool, MigrateError> {
    let left_bytes =
        storage::fs_read(left).map_err(|err| map_io(left, err, storage::IoAction::Read))?;
    let right_bytes =
        storage::fs_read(right).map_err(|err| map_io(right, err, storage::IoAction::Read))?;
    Ok(left_bytes == right_bytes)
}

fn remove_copied(paths: &[PathBuf]) -> bool {
    let mut ok = true;
    for path in paths.iter().rev() {
        if storage::fs_remove_file(path).is_err() && storage::fs_exists(path) {
            ok = false;
        }
    }
    ok
}

fn probe_writable(dir: &Path) -> Result<(), MigrateError> {
    let probe = dir.join(".lanwork-write-probe");
    storage::atomic_write(&probe, b"ok")
        .map_err(|err| map_io(&probe, err, storage::IoAction::Write))?;
    if storage::fs_remove_file(&probe).is_err() && storage::fs_exists(&probe) {
        return Err(MigrateError::Io {
            message: format!("无法写入：{}，仍使用原来的目录", probe.display()),
        });
    }
    Ok(())
}

fn ensure_space(
    target: &Path,
    source: &Path,
    names: &[String],
    force_no_space: bool,
) -> Result<(), MigrateError> {
    if force_no_space {
        return Err(MigrateError::NoSpace);
    }
    let mut need = 0u64;
    for name in names {
        let len = storage::fs_file_len(&join(source, name))
            .map_err(|err| map_io(&join(source, name), err, storage::IoAction::Read))?;
        need = need.saturating_add(len);
    }
    if let Some(free) = free_bytes(target) {
        // 目录项和临时文件还要占一点。预检少算时，复制失败仍会停在旧目录。
        let margin = 1024 * 1024;
        if free < need.saturating_add(margin) {
            return Err(MigrateError::NoSpace);
        }
    }
    Ok(())
}

/// 导出白名单，加上存在的清除记录。两边用同一份名单比较，字节一致才能补写引导文件。
fn migration_names(dir: &Path) -> Result<Vec<String>, MigrateError> {
    let mut names = data_file_names(dir).map_err(|err| MigrateError::Io {
        message: format!("{err}，仍使用原来的目录"),
    })?;
    if storage::fs_exists(&join(dir, PURGE_PENDING)) {
        names.push(PURGE_PENDING.to_owned());
    }
    names.sort();
    Ok(names)
}

fn has_lanwork_data(dir: &Path) -> Result<bool, MigrateError> {
    if !migration_names(dir)?.is_empty() {
        return Ok(true);
    }
    for name in [
        "reminder-sent.json",
        "import.pending",
        "github/watchlist.json",
        "tool-enabled.json",
    ] {
        if storage::fs_exists(&join(dir, name)) {
            return Ok(true);
        }
    }
    for name in [
        "github/cache",
        "backups",
        "logs",
        "cache/web-icons",
        "tool-storage",
        "tool-windows",
    ] {
        if dir_has_file(&join(dir, name)) {
            return Ok(true);
        }
    }
    if nested_has_file(&join(dir, "tools"), 0) {
        return Ok(true);
    }
    for name in ["", "todos", "notes", "shelves"] {
        if has_corrupt(&join(dir, name)) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn nested_has_file(dir: &Path, depth: u32) -> bool {
    if depth > 32 {
        return true;
    }
    let Ok(entries) = storage::fs_read_dir(dir) else {
        return false;
    };
    for entry in entries {
        if entry.is_file {
            return true;
        }
        let real_dir =
            std::fs::symlink_metadata(&entry.path).is_ok_and(|meta| meta.file_type().is_dir());
        if real_dir && nested_has_file(&entry.path, depth + 1) {
            return true;
        }
    }
    false
}

fn dir_has_file(dir: &Path) -> bool {
    storage::fs_read_dir(dir)
        .ok()
        .is_some_and(|entries| entries.iter().any(|entry| entry.is_file))
}

fn has_corrupt(dir: &Path) -> bool {
    storage::fs_read_dir(dir).ok().is_some_and(|entries| {
        entries.iter().any(|entry| {
            entry
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".corrupt-"))
        })
    })
}

fn join(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    if !relative.is_empty() {
        for part in relative.split('/') {
            path.push(part);
        }
    }
    path
}

fn map_io(path: &Path, err: std::io::Error, action: storage::IoAction) -> MigrateError {
    if is_no_space(&err) {
        MigrateError::NoSpace
    } else {
        io_message(storage::Error::io(action, path, err))
    }
}

fn io_message(err: storage::Error) -> MigrateError {
    match &err {
        storage::Error::MayDolist { .. } => MigrateError::Io {
            message: err.to_string(),
        },
        _ => MigrateError::Io {
            message: format!("{err}，仍使用原来的目录"),
        },
    }
}

fn is_no_space(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::StorageFull
        || matches!(err.raw_os_error(), Some(28 | 112 | 39))
}

#[cfg(windows)]
fn free_bytes(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut free = 0u64;
    // SAFETY: wide 以 0 结尾，free 是可写的 u64。失败时返回 0，调用方改靠复制时的错误。
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if ok == 0 { None } else { Some(free) }
}

#[cfg(not(windows))]
fn free_bytes(_path: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use super::*;
    use crate::storage::{
        DataDirSource, DocumentId, ExampleDocument, ResolveInput, SCHEMA_VERSION, Store,
        StorePaths, maydolist_dir, resolve, test_temp::TempDir,
    };

    fn open(root: &Path) -> Store {
        Store::open(StorePaths {
            data_dir: root.join("data"),
            cache_dir: root.join("cache"),
            user_profile: root.join("profile"),
            local_app_data: root.join("local"),
        })
        .unwrap()
    }

    fn note(id: &str, title: &str) -> ExampleDocument {
        ExampleDocument {
            schema_version: SCHEMA_VERSION,
            id: id.to_owned(),
            title: title.to_owned(),
            pinned: false,
        }
    }

    fn seed(store: &Store) {
        store
            .write_json(&DocumentId::Note("n1".into()), &note("n1", "一条"))
            .unwrap();
        let dir = store.data_dir();
        std::fs::write(
            dir.join("config.json"),
            b"{\"schemaVersion\":1,\"dataDir\":\"old\"}\n",
        )
        .unwrap();
        std::fs::write(dir.join("user-apps.json"), b"{\"schemaVersion\":1}\n").unwrap();
        std::fs::write(dir.join("todo-float.json"), b"{\"schemaVersion\":1}\n").unwrap();
        std::fs::write(
            dir.join("reminder-sent.json"),
            b"{\"schemaVersion\":1,\"sent\":[]}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("todo-purge-pending.json"),
            b"{\"schemaVersion\":1,\"ids\":[\"x\"]}\n",
        )
        .unwrap();
        let icons = dir.join("cache").join("web-icons");
        std::fs::create_dir_all(&icons).unwrap();
        std::fs::write(icons.join("ab.bin"), b"icon").unwrap();
        std::fs::write(dir.join("backups").join("manual-1.zip"), b"zip").unwrap();
        store.log_info("stay-behind");
        std::fs::create_dir_all(dir.join("github").join("cache")).unwrap();
        std::fs::write(dir.join("github").join("watchlist.json"), b"{}").unwrap();
        std::fs::write(dir.join("github").join("cache").join("a.json"), b"{}").unwrap();
        std::fs::write(dir.join("notes").join("x.json.corrupt-1-1"), b"{").unwrap();
        let tool = dir.join("tools").join("example.quota");
        std::fs::create_dir_all(&tool).unwrap();
        std::fs::write(tool.join("manifest.toml"), b"tool-copy\n").unwrap();
        std::fs::write(tool.join("grant.json"), b"{}\n").unwrap();
        for scratch in [
            ".incoming-example.quota",
            ".previous-example.quota",
            ".hold-example.quota.0",
        ] {
            let path = dir.join("tools").join(scratch);
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("leak.txt"), b"scratch").unwrap();
        }
        std::fs::create_dir_all(dir.join("tool-storage")).unwrap();
        std::fs::write(
            dir.join("tool-storage").join("example.quota.json"),
            b"{\"schemaVersion\":1,\"entries\":{}}\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("tool-windows")).unwrap();
        std::fs::write(
            dir.join("tool-windows").join("example.quota.json"),
            b"{\"schemaVersion\":1,\"open\":true}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("tool-windows")
                .join("lanwork.float-shell-demo.json"),
            b"{\"schemaVersion\":1,\"open\":true}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("tool-enabled.json"),
            b"{\"schemaVersion\":1,\"disabled\":[]}\n",
        )
        .unwrap();
    }

    fn resolved(root: &Path) -> crate::storage::ResolvedPaths {
        resolve(&ResolveInput {
            user_profile: root.join("profile"),
            local_app_data: root.join("local"),
            env_data_dir: None,
            current_dir: root.to_path_buf(),
        })
        .unwrap()
    }

    fn run(store: &Store, target: &Path) -> Result<PathBuf, MigrateError> {
        migrate_data_dir(store, target, false, |_| {})
    }

    #[test]
    fn success_copies_the_allowlist_switches_and_keeps_the_old_directory() {
        let temp = TempDir::new();
        let store = open(temp.path());
        seed(&store);
        let old = store.data_dir();
        let target = temp.path().join("next");
        let mut phases = Vec::new();
        let switched = migrate(&store, &target, false, Fault::default(), |progress| {
            phases.push(progress.phase)
        })
        .unwrap();
        assert_eq!(switched, target);
        assert_eq!(store.data_dir(), target);
        assert!(phases.contains(&MigratePhase::Copy));
        assert!(phases.contains(&MigratePhase::Verify));
        assert!(phases.contains(&MigratePhase::Switch));
        for name in [
            "config.json",
            "user-apps.json",
            "todo-float.json",
            "notes/n1.json",
            "todo-purge-pending.json",
            "tools/example.quota/manifest.toml",
            "tools/example.quota/grant.json",
            "tool-storage/example.quota.json",
            "tool-windows/example.quota.json",
            "tool-enabled.json",
        ] {
            let from = super::join(&old, name);
            let dest = super::join(&target, name);
            assert_eq!(
                std::fs::read(&from).unwrap(),
                std::fs::read(&dest).unwrap(),
                "{name}"
            );
        }
        assert!(
            std::fs::read_to_string(target.join("config.json"))
                .unwrap()
                .contains("\"dataDir\":\"old\"")
        );
        for name in [
            "reminder-sent.json",
            "cache/web-icons/ab.bin",
            "backups/manual-1.zip",
            "logs/app.log",
            "github/watchlist.json",
            "github/cache/a.json",
            "notes/x.json.corrupt-1-1",
            "tools/.incoming-example.quota/leak.txt",
            "tools/.previous-example.quota/leak.txt",
            "tools/.hold-example.quota.0/leak.txt",
            "tool-windows/lanwork.float-shell-demo.json",
        ] {
            assert!(
                !super::join(&target, name).exists(),
                "{name} should stay behind"
            );
            assert!(
                super::join(&old, name).exists(),
                "{name} removed from the old directory"
            );
        }
        assert!(old.is_dir());
        let again = resolved(temp.path());
        assert_eq!(again.source, DataDirSource::Bootstrap);
        assert_eq!(again.data_dir, target);

        store
            .write_json(&DocumentId::Note("n1".into()), &note("n1", "改过"))
            .unwrap();
        let updated = std::fs::read_to_string(target.join("notes").join("n1.json")).unwrap();
        assert!(updated.contains("改过"));
        assert!(
            !std::fs::read_to_string(old.join("notes").join("n1.json"))
                .unwrap()
                .contains("改过")
        );
    }

    #[test]
    fn writes_during_migration_land_in_the_new_directory() {
        let temp = TempDir::new();
        let store = open(temp.path());
        seed(&store);
        let old = store.data_dir();
        let target = temp.path().join("next");
        let store2 = store.clone();
        let started = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&started);
        let writer = std::thread::spawn(move || {
            while !flag.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
            store2
                .write_json(&DocumentId::Note("later".into()), &note("later", "之后"))
                .unwrap();
        });
        run_until(&store, &target, |progress| {
            if progress.phase == MigratePhase::Copy && progress.done == 0 {
                started.store(true, Ordering::Release);
                std::thread::sleep(Duration::from_millis(30));
            }
        })
        .unwrap();
        writer.join().unwrap();
        assert!(target.join("notes").join("later.json").is_file());
        assert!(!old.join("notes").join("later.json").exists());
    }

    #[test]
    fn tool_storage_written_after_the_switch_lands_in_the_new_directory() {
        let temp = TempDir::new();
        let store = open(temp.path());
        seed(&store);
        let old = store.data_dir();
        let target = temp.path().join("next");
        run(&store, &target).unwrap();
        let mut entries = std::collections::BTreeMap::new();
        entries.insert("note".to_owned(), "after-switch".to_owned());
        crate::panel::write_tool_storage(&store.data_dir(), "example.quota", &entries).unwrap();
        let written =
            std::fs::read_to_string(target.join("tool-storage/example.quota.json")).unwrap();
        assert!(written.contains("after-switch"));
        assert!(
            !std::fs::read_to_string(old.join("tool-storage/example.quota.json"))
                .unwrap()
                .contains("after-switch")
        );
    }

    fn run_until(
        store: &Store,
        target: &Path,
        progress: impl FnMut(MigrateProgress),
    ) -> Result<PathBuf, MigrateError> {
        migrate(store, target, false, Fault::default(), progress)
    }

    #[test]
    fn stopping_during_copy_keeps_the_old_directory_and_blocks_a_partial_retry() {
        let temp = TempDir::new();
        let store = open(temp.path());
        seed(&store);
        let old = store.data_dir();
        let target = temp.path().join("next");
        let err = migrate(
            &store,
            &target,
            false,
            Fault {
                die_after_copies: Some(1),
                ..Fault::default()
            },
            |_| {},
        )
        .unwrap_err();
        assert!(matches!(err, MigrateError::Stopped));
        assert_eq!(store.data_dir(), old);
        assert_eq!(resolved(temp.path()).source, DataDirSource::Default);
        assert!(old.join("notes").join("n1.json").is_file());
        assert!(target.exists());
        let retry = run(&store, &target).unwrap_err();
        assert!(matches!(retry, MigrateError::AlreadyHasData));
        assert_eq!(store.data_dir(), old);
        assert!(old.join("config.json").is_file());
    }

    #[test]
    fn verify_failure_stays_on_the_old_directory_and_removes_the_partial_copy() {
        let temp = TempDir::new();
        let store = open(temp.path());
        seed(&store);
        let old = store.data_dir();
        let before = std::fs::read(old.join("notes").join("n1.json")).unwrap();
        let target = temp.path().join("next");
        let err = migrate(
            &store,
            &target,
            false,
            Fault {
                corrupt_first: true,
                ..Fault::default()
            },
            |_| {},
        )
        .unwrap_err();
        assert!(matches!(
            err,
            MigrateError::VerifyFailed { leftovers: false }
        ));
        assert_eq!(err.to_string(), "校验失败，仍使用原来的目录");
        assert_eq!(store.data_dir(), old);
        assert_eq!(resolved(temp.path()).source, DataDirSource::Default);
        assert_eq!(
            std::fs::read(old.join("notes").join("n1.json")).unwrap(),
            before
        );
        assert!(!target.join("config.json").exists());
        assert!(!target.join("notes").join("n1.json").exists());
        assert!(!target.join("user-apps.json").exists());
        assert!(!target.join("todo-float.json").exists());
    }

    #[test]
    fn a_verified_copy_without_bootstrap_can_be_finished_later() {
        let temp = TempDir::new();
        let store = open(temp.path());
        seed(&store);
        let old = store.data_dir();
        let target = temp.path().join("next");
        let err = migrate(
            &store,
            &target,
            false,
            Fault {
                skip_bootstrap: true,
                ..Fault::default()
            },
            |_| {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("仍使用原来的目录"));
        assert_eq!(store.data_dir(), old);
        assert_eq!(resolved(temp.path()).source, DataDirSource::Default);
        assert_eq!(
            std::fs::read(old.join("notes").join("n1.json")).unwrap(),
            std::fs::read(target.join("notes").join("n1.json")).unwrap()
        );
        run(&store, &target).unwrap();
        assert_eq!(store.data_dir(), target);
        assert_eq!(resolved(temp.path()).data_dir, target);
        assert!(old.join("notes").join("n1.json").is_file());
    }

    #[test]
    fn purge_pending_follows_the_list_and_shelf_and_must_match_to_resume() {
        let temp = TempDir::new();
        let store = open(temp.path());
        seed(&store);
        let old = store.data_dir();
        let list = br#"{"schemaVersion":1,"id":"inbox","items":[]}"#;
        let shelf = br#"{"schemaVersion":1,"id":"g","todoIds":["gone"]}"#;
        let purge = br#"{"schemaVersion":1,"ids":["gone"]}"#;
        std::fs::write(old.join("todos").join("inbox.json"), list).unwrap();
        std::fs::create_dir_all(old.join("shelves")).unwrap();
        std::fs::write(old.join("shelves").join("g.json"), shelf).unwrap();
        std::fs::write(old.join("todo-purge-pending.json"), purge).unwrap();
        let target = temp.path().join("next");
        let err = migrate(
            &store,
            &target,
            false,
            Fault {
                skip_bootstrap: true,
                ..Fault::default()
            },
            |_| {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("仍使用原来的目录"));
        assert_eq!(store.data_dir(), old);
        for (name, bytes) in [
            ("todos/inbox.json", list.as_slice()),
            ("shelves/g.json", shelf.as_slice()),
            ("todo-purge-pending.json", purge.as_slice()),
        ] {
            assert_eq!(std::fs::read(super::join(&target, name)).unwrap(), bytes);
            assert_eq!(std::fs::read(super::join(&old, name)).unwrap(), bytes);
        }
        let mut flipped = purge.to_vec();
        flipped[0] ^= 0xff;
        std::fs::write(target.join("todo-purge-pending.json"), &flipped).unwrap();
        let err = run(&store, &target).unwrap_err();
        assert!(matches!(err, MigrateError::AlreadyHasData));
        assert_eq!(store.data_dir(), old);
        assert_eq!(
            std::fs::read(target.join("todo-purge-pending.json")).unwrap(),
            flipped
        );
        assert_eq!(
            std::fs::read(old.join("todo-purge-pending.json")).unwrap(),
            purge
        );
        std::fs::write(target.join("todo-purge-pending.json"), purge).unwrap();
        run(&store, &target).unwrap();
        assert_eq!(store.data_dir(), target);
        assert_eq!(resolved(temp.path()).data_dir, target);
        assert_eq!(
            std::fs::read(target.join("shelves").join("g.json")).unwrap(),
            shelf
        );
        assert_eq!(
            std::fs::read(target.join("todo-purge-pending.json")).unwrap(),
            purge
        );
        assert!(old.join("todo-purge-pending.json").is_file());
    }

    #[test]
    fn different_existing_data_is_refused_and_not_deleted() {
        let temp = TempDir::new();
        let store = open(temp.path());
        seed(&store);
        let old = store.data_dir();
        let target = temp.path().join("next");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(
            target.join("config.json"),
            b"{\"schemaVersion\":1,\"dataDir\":\"other\"}\n",
        )
        .unwrap();
        let err = run(&store, &target).unwrap_err();
        assert!(matches!(err, MigrateError::AlreadyHasData));
        assert_eq!(
            std::fs::read(target.join("config.json")).unwrap(),
            b"{\"schemaVersion\":1,\"dataDir\":\"other\"}\n"
        );
        assert_eq!(store.data_dir(), old);
        assert!(!target.join("notes").join("n1.json").exists());
        assert_eq!(resolved(temp.path()).source, DataDirSource::Default);
    }

    #[test]
    fn no_space_env_same_nested_maydolist_and_import_pending_do_not_switch() {
        let temp = TempDir::new();
        let store = open(temp.path());
        seed(&store);
        let old = store.data_dir();
        let target = temp.path().join("full");
        let err = migrate(
            &store,
            &target,
            false,
            Fault {
                force_no_space: true,
                ..Fault::default()
            },
            |_| {},
        )
        .unwrap_err();
        assert!(matches!(err, MigrateError::NoSpace));
        assert_eq!(err.to_string(), "空间不足，仍使用原来的目录");
        assert_eq!(store.data_dir(), old);
        assert!(!target.join("config.json").exists());

        let err = migrate_data_dir(&store, &temp.path().join("env"), true, |_| {}).unwrap_err();
        assert!(matches!(err, MigrateError::EnvBlocked));
        assert!(!temp.path().join("env").exists());

        let err = run(&store, &old).unwrap_err();
        assert!(matches!(err, MigrateError::SameDirectory));

        let err = run(&store, &old.join("inside")).unwrap_err();
        assert!(matches!(err, MigrateError::Nested));
        let err = run(&store, temp.path()).unwrap_err();
        assert!(matches!(err, MigrateError::Nested));

        let forbidden = maydolist_dir(&temp.path().join("profile"));
        let err = run(&store, &forbidden).unwrap_err();
        assert!(err.to_string().contains("不能使用该数据目录"));
        assert!(!forbidden.exists());

        std::fs::write(old.join("import.pending"), b"C:\\backup.zip\n").unwrap();
        let err = run(&store, &temp.path().join("after-import")).unwrap_err();
        assert!(matches!(err, MigrateError::ImportPending));
        assert_eq!(store.data_dir(), old);
        assert_eq!(resolved(temp.path()).source, DataDirSource::Default);
    }

    #[test]
    fn a_file_or_unwritable_parent_stays_on_the_old_directory() {
        let temp = TempDir::new();
        let store = open(temp.path());
        seed(&store);
        let old = store.data_dir();
        let file = temp.path().join("not-a-dir");
        std::fs::write(&file, b"x").unwrap();
        let err = run(&store, &file).unwrap_err();
        assert!(matches!(err, MigrateError::NotADirectory { .. }));
        assert_eq!(std::fs::read(&file).unwrap(), b"x");
        let err = run(&store, &file.join("child")).unwrap_err();
        assert!(matches!(err, MigrateError::Io { .. }));
        assert!(err.to_string().contains("仍使用原来的目录"));
        assert_eq!(store.data_dir(), old);
    }

    #[test]
    fn allowlist_matches_the_package_and_skips_reminder_sent() {
        let temp = TempDir::new();
        let store = open(temp.path());
        seed(&store);
        let listed = super::super::package::list_data_files(&store).unwrap();
        let mut from_package: Vec<_> = listed.into_iter().map(|(name, _)| name).collect();
        from_package.sort();
        let mut from_names = super::super::package::data_file_names(&store.data_dir()).unwrap();
        from_names.sort();
        assert_eq!(from_package, from_names);
        assert!(from_names.iter().any(|name| name == "notes/n1.json"));
        assert!(!from_names.iter().any(|name| name.contains("reminder-sent")));
        assert!(!from_names.iter().any(|name| name.contains("web-icons")));
        assert!(!from_names.iter().any(|name| name.contains("todo-purge")));
        let copied = migration_names(&store.data_dir()).unwrap();
        assert!(copied.iter().any(|name| name == "todo-purge-pending.json"));
        assert_eq!(
            progress_text(MigrateProgress {
                phase: MigratePhase::Copy,
                done: 1,
                total: 4,
            }),
            "正在复制 1/4"
        );
        assert_eq!(
            progress_text(MigrateProgress {
                phase: MigratePhase::Switch,
                done: 0,
                total: 4,
            }),
            "正在切换"
        );
    }
}
