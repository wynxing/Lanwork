use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::WindowsSources;
use super::app_paths::read_app_paths;
use super::path_env::read_path_entries;
use super::shortcut::save_shortcut;
use super::start_menu::read_shortcut_dir_excluding;
use super::store::read_store;
use crate::apps::index::SourceEnumerator;
use crate::apps::rules::is_package_aumid;
use crate::apps::{AppSource, LaunchTarget, OpenOptions, launch_and_wait, launch_key, open_with};
use crate::storage::test_temp::TempDir;

#[test]
fn start_menu_finds_an_app() {
    let mut sources = WindowsSources::new(None);
    let entries = sources.enumerate(AppSource::StartMenu).expect("start menu");
    assert!(
        entries
            .iter()
            .any(|entry| entry.source == AppSource::StartMenu),
        "开始菜单里应该至少有一个目标存在的快捷方式"
    );
}

#[test]
fn store_entries_are_aumids_and_include_calculator_when_installed() {
    let entries = read_store().expect("store");
    for entry in &entries {
        assert_eq!(entry.source, AppSource::Store);
        assert!(!entry.name.is_empty(), "商店应用应该有显示名");
        match &entry.target {
            LaunchTarget::Aumid { aumid } => {
                assert!(is_package_aumid(aumid), "商店来源只收打包应用: {aumid}");
            }
            LaunchTarget::Path { .. } | LaunchTarget::Url { .. } => {
                panic!("商店应用不应该用路径或协议目标")
            }
        }
    }
    // GitHub 托管的 Windows 镜像通常没有计算器。本机装了才要求命中 AUMID。
    let installed = std::env::var_os("LOCALAPPDATA").is_some_and(|dir| {
        PathBuf::from(dir)
            .join("Packages")
            .join("Microsoft.WindowsCalculator_8wekyb3d8bbwe")
            .is_dir()
    });
    if installed {
        assert!(
            entries.iter().any(|entry| {
                matches!(&entry.target, LaunchTarget::Aumid { aumid } if aumid.to_lowercase().contains("windowscalculator"))
            }),
            "已安装的计算器应该以 AUMID 进入商店索引"
        );
    }
}

#[test]
fn path_finds_notepad() {
    let entries = read_path_entries().expect("path");
    assert!(
        entries
            .iter()
            .any(|entry| target_ends_with(&entry.target, r"\notepad.exe")),
        "PATH 里应该有 notepad.exe"
    );
}

#[test]
fn start_menu_and_app_paths_collapse_to_one_launch_target() {
    let paths = read_app_paths().expect("app paths");
    let mut sources = WindowsSources::new(None);
    let start = sources.enumerate(AppSource::StartMenu).expect("start menu");
    let natural = shared_keys(&start, &paths);
    let sample = paths
        .iter()
        .find(|entry| matches!(entry.target, LaunchTarget::Path { .. }))
        .expect("at least one app path");
    let LaunchTarget::Path { path: target, .. } = &sample.target else {
        unreachable!();
    };
    let temp = TempDir::new();
    save_shortcut(&temp.path().join("Same Program.lnk"), target, "", None).unwrap();
    let mut with_extra = WindowsSources::new(Some(temp.path().to_path_buf()));
    let start_with_extra = with_extra
        .enumerate(AppSource::StartMenu)
        .expect("start menu plus extra");
    let controlled = shared_keys(&start_with_extra, &paths);
    assert!(
        !controlled.is_empty(),
        "测量目录里的快捷方式和 App Paths 应该指向同一个程序"
    );
    let merged = crate::apps::model::dedupe_entries(&[&start_with_extra, &paths]);
    let counts = key_counts(&merged);
    for key in controlled.iter().chain(natural.iter()) {
        assert_eq!(counts.get(key).copied().unwrap_or(0), 1, "{key}");
    }
    assert!(
        merged
            .iter()
            .any(|entry| { entry.name == "Same Program" && entry.source == AppSource::StartMenu }),
        "同目标时保留开始菜单里的名称"
    );
}

#[test]
fn shortcut_keeps_args_and_skips_missing_targets() {
    let temp = TempDir::new();
    let cmd = PathBuf::from(r"C:\Windows\System32\cmd.exe");
    save_shortcut(
        &temp.path().join("With Args.lnk"),
        &cmd,
        "--kept",
        Some(temp.path()),
    )
    .unwrap();
    save_shortcut(
        &temp.path().join("Broken.lnk"),
        Path::new(r"C:\Lanwork\missing\not-here.exe"),
        "",
        None,
    )
    .unwrap();
    let broken_aumid = temp.path().join("Store Tile.lnk");
    save_shortcut(
        &broken_aumid,
        Path::new(r"C:\Lanwork\missing\store-tile.exe"),
        "",
        None,
    )
    .unwrap();
    super::shortcut::stamp_aumid(
        &broken_aumid,
        "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App",
    )
    .unwrap();
    assert_eq!(
        super::shortcut::read_aumid(&broken_aumid).as_deref(),
        Some("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App")
    );
    let entries = read_shortcut_dir_excluding(temp.path(), &[]).unwrap();
    assert_eq!(entries.len(), 1);
    match &entries[0].target {
        LaunchTarget::Path {
            path,
            args,
            working_directory,
        } => {
            assert!(path.to_string_lossy().to_lowercase().ends_with(r"\cmd.exe"));
            assert_eq!(args, "--kept");
            assert_eq!(working_directory.as_deref(), Some(temp.path()));
        }
        LaunchTarget::Aumid { .. } => panic!("cmd shortcut should be a path"),
        LaunchTarget::Url { .. } => panic!("cmd shortcut should be a path"),
    }
    assert!(
        entries[0]
            .alternate_names
            .iter()
            .any(|name| name.eq_ignore_ascii_case("cmd.exe"))
            || entries[0].name.eq_ignore_ascii_case("cmd.exe")
    );
    assert!(
        entries[0].name.eq_ignore_ascii_case("With Args")
            || entries[0]
                .alternate_names
                .iter()
                .any(|name| name.to_lowercase().contains("with args"))
    );
}

#[test]
fn game_url_is_indexed_and_web_url_is_not() {
    let temp = TempDir::new();
    std::fs::write(
        temp.path().join("Game.url"),
        "[InternetShortcut]\r\nURL=steam://rungameid/570\r\n",
    )
    .unwrap();
    std::fs::write(
        temp.path().join("Site.url"),
        "[InternetShortcut]\r\nURL=https://example.com/\r\n",
    )
    .unwrap();
    let entries = read_shortcut_dir_excluding(temp.path(), &[]).unwrap();
    assert_eq!(entries.len(), 1);
    match &entries[0].target {
        LaunchTarget::Url { url } => assert_eq!(url, "steam://rungameid/570"),
        LaunchTarget::Path { .. } | LaunchTarget::Aumid { .. } => {
            panic!("game shortcut should be a url")
        }
    }
}

#[test]
fn uninstall_shortcut_is_dropped_and_only_the_known_startup_path_is_skipped() {
    let temp = TempDir::new();
    let cmd = PathBuf::from(r"C:\Windows\System32\cmd.exe");
    save_shortcut(&temp.path().join("Keeper.lnk"), &cmd, "", None).unwrap();
    save_shortcut(&temp.path().join("Uninstall Helper.lnk"), &cmd, "", None).unwrap();
    let startup = temp.path().join("启动");
    std::fs::create_dir_all(&startup).unwrap();
    save_shortcut(&startup.join("Hidden.lnk"), &cmd, "", None).unwrap();
    let named = temp.path().join("Startup");
    std::fs::create_dir_all(&named).unwrap();
    save_shortcut(&named.join("Still Here.lnk"), &cmd, "", None).unwrap();
    let entries =
        super::start_menu::read_shortcut_dir_excluding(temp.path(), std::slice::from_ref(&startup))
            .unwrap();
    assert!(entries.iter().any(|entry| mentions(entry, "keeper")));
    assert!(entries.iter().any(|entry| mentions(entry, "still here")));
    assert!(entries.iter().all(|entry| !mentions(entry, "uninstall")));
    assert!(entries.iter().all(|entry| !mentions(entry, "hidden")));
}

#[test]
fn known_startup_folders_do_not_exclude_a_decoy_directory() {
    let excluded = super::start_menu::startup_folders();
    for folder in &excluded {
        assert!(crate::apps::rules::is_excluded_directory(folder, &excluded));
    }
    let temp = TempDir::new();
    let decoy = temp.path().join("启动");
    std::fs::create_dir_all(&decoy).unwrap();
    assert!(!crate::apps::rules::is_excluded_directory(
        &decoy, &excluded
    ));
    let startup = temp.path().join("Startup");
    std::fs::create_dir_all(&startup).unwrap();
    assert!(!crate::apps::rules::is_excluded_directory(
        &startup, &excluded
    ));
}

#[test]
fn missing_extra_directory_does_not_drop_the_real_start_menu() {
    let mut sources = WindowsSources::new(Some(PathBuf::from(r"C:\Windows\System32\notepad.exe")));
    let entries = sources.enumerate(AppSource::StartMenu).expect("partial");
    assert!(
        entries
            .iter()
            .any(|entry| entry.source == AppSource::StartMenu)
    );
}

#[test]
fn directory_watch_adds_a_shortcut_from_the_extra_directory() {
    let temp = TempDir::new();
    let cache = temp.path().join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    let extra = temp.path().join("shortcuts");
    std::fs::create_dir_all(&extra).unwrap();
    let index = open_with(OpenOptions {
        cache_path: cache.join("apps.json"),
        extra_shortcut_dir: None,
        watch: true,
        background_rebuild: true,
        debounce: Duration::from_millis(200),
        enumerator: Some(Box::new(ExtraDir(extra.clone()))),
        cache_log: None,
        user_catalog: crate::apps::UserCatalog::default(),
    })
    .unwrap();
    index.wait_idle().expect("initial rebuild");
    assert!(index.query("Lanwork Watch Probe").is_empty());
    save_shortcut(
        &extra.join("Lanwork Watch Probe.lnk"),
        Path::new(r"C:\Windows\System32\cmd.exe"),
        "--lanwork-watch-probe",
        None,
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Some(hit) = index.query("Lanwork Watch Probe").into_iter().next() {
            match &hit.entry.target {
                LaunchTarget::Path { args, .. } => assert_eq!(args, "--lanwork-watch-probe"),
                LaunchTarget::Aumid { .. } | LaunchTarget::Url { .. } => {
                    panic!("监视到的快捷方式应该是路径目标")
                }
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    panic!("监视开始菜单来源后，新快捷方式没有进入索引");
}

#[test]
fn launch_passes_args_and_working_directory_to_cmd() {
    let temp = TempDir::new();
    std::fs::write(temp.path().join("probe.txt"), b"ok").unwrap();
    let target = LaunchTarget::Path {
        path: PathBuf::from(r"C:\Windows\System32\cmd.exe"),
        args: "/c if exist probe.txt (exit 4) else (exit 5)".to_owned(),
        working_directory: Some(temp.path().to_path_buf()),
    };
    let code = launch_and_wait(&target, 15_000).expect("cmd");
    assert_eq!(code, 4);
}

#[test]
#[ignore = "测量 5000 个快捷方式，不放进 CI"]
fn measure_real_machine_and_five_thousand_shortcuts() {
    let before = private_bytes();
    let started = Instant::now();
    let mut sources = WindowsSources::new(None);
    let mut counts = HashMap::new();
    for source in AppSource::ALL {
        let entries = sources.enumerate(source).expect("source");
        counts.insert(format!("{source:?}"), entries.len());
    }
    let real_elapsed = started.elapsed();
    let after_real = private_bytes();
    let temp = TempDir::new();
    let fixture = temp.path().join("fixture");
    std::fs::create_dir_all(&fixture).unwrap();
    let cmd = Path::new(r"C:\Windows\System32\cmd.exe");
    let build_started = Instant::now();
    for index in 0..5_000 {
        save_shortcut(
            &fixture.join(format!("Fixture {index}.lnk")),
            cmd,
            &format!("--n {index}"),
            None,
        )
        .unwrap();
    }
    let build_elapsed = build_started.elapsed();
    let scan_started = Instant::now();
    let scanned = read_shortcut_dir_excluding(&fixture, &[]).unwrap();
    let scan_elapsed = scan_started.elapsed();
    let after = private_bytes();
    eprintln!(
        "real_sources={counts:?} real_elapsed_ms={} private_before={before} private_after_real={after_real} fixture_build_ms={} fixture_entries={} fixture_scan_ms={} private_after={after}",
        real_elapsed.as_millis(),
        build_elapsed.as_millis(),
        scanned.len(),
        scan_elapsed.as_millis(),
    );
    assert_eq!(scanned.len(), 5_000);
}

struct ExtraDir(PathBuf);

impl SourceEnumerator for ExtraDir {
    fn enumerate(&mut self, source: AppSource) -> Result<Vec<crate::apps::AppEntry>, String> {
        if source == AppSource::StartMenu {
            read_shortcut_dir_excluding(&self.0, &[])
        } else {
            Ok(Vec::new())
        }
    }

    fn watch_directories(&self) -> Vec<PathBuf> {
        vec![self.0.clone()]
    }
}

fn mentions(entry: &crate::apps::AppEntry, needle: &str) -> bool {
    let needle = needle.to_lowercase();
    std::iter::once(entry.name.as_str())
        .chain(entry.alternate_names.iter().map(String::as_str))
        .any(|name| name.to_lowercase().contains(&needle))
}

fn target_ends_with(target: &LaunchTarget, suffix: &str) -> bool {
    match target {
        LaunchTarget::Path { path, .. } => path.to_string_lossy().to_lowercase().ends_with(suffix),
        LaunchTarget::Aumid { .. } | LaunchTarget::Url { .. } => false,
    }
}

fn shared_keys(left: &[crate::apps::AppEntry], right: &[crate::apps::AppEntry]) -> Vec<String> {
    let keys: std::collections::HashSet<_> =
        left.iter().map(|entry| launch_key(&entry.target)).collect();
    right
        .iter()
        .map(|entry| launch_key(&entry.target))
        .filter(|key| keys.contains(key))
        .collect()
}

fn key_counts(entries: &[crate::apps::AppEntry]) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for entry in entries {
        *counts.entry(launch_key(&entry.target)).or_insert(0) += 1;
    }
    counts
}

fn private_bytes() -> u64 {
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
    use windows::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
        let size = u32::try_from(std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>()).unwrap_or(0);
        counters.cb = size;
        let ok = GetProcessMemoryInfo(GetCurrentProcess(), &mut counters as *mut _ as *mut _, size);
        if ok.is_ok() {
            u64::try_from(counters.PrivateUsage).unwrap_or(0)
        } else {
            0
        }
    }
}
