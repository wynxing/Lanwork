//! 应用条目、启动目标和去重。
//!
//! 去重键是启动目标：规范化路径加参数、AUMID，或协议地址。工作目录不进键。
//! 同一键保留先出现的一条。来源顺序是开始菜单、App Paths、PATH、商店应用、
//! 便携应用、别名。别名不单独成条，只把名称并入已有条目的备用名。

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// 一条可搜索的应用。`source` 是合并后留下来的那一次来源。
/// `alternate_names` 可以搜到，不作为结果上的名称。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppEntry {
    pub name: String,
    pub source: AppSource,
    pub target: LaunchTarget,
    pub icon_path: Option<PathBuf>,
    pub icon_index: i32,
    pub alternate_names: Vec<String>,
}

/// 索引来源。`Portable` 和 `Alias` 来自用户目录，不由系统枚举产生。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppSource {
    StartMenu,
    AppPaths,
    Path,
    Store,
    Portable,
    Alias,
}

impl AppSource {
    pub(crate) const ALL: [Self; 6] = [
        Self::StartMenu,
        Self::AppPaths,
        Self::Path,
        Self::Store,
        Self::Portable,
        Self::Alias,
    ];
}

/// 启动目标。路径目标保留参数和工作目录，交给 `ShellExecuteExW`。
/// 商店应用只保留 AUMID。协议链接保留收录的地址。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LaunchTarget {
    Path {
        path: PathBuf,
        args: String,
        working_directory: Option<PathBuf>,
    },
    Aumid {
        aumid: String,
    },
    Url {
        url: String,
    },
}

/// 按启动目标生成去重键。空路径或空 AUMID 得到空字符串，调用方丢掉这条。
#[must_use]
pub fn launch_key(target: &LaunchTarget) -> String {
    match target {
        LaunchTarget::Path { path, args, .. } => {
            let path = normalize_path_key(path);
            if path.is_empty() {
                return String::new();
            }
            let args = args.trim();
            if args.is_empty() {
                format!("path\u{1}{path}")
            } else {
                format!("path\u{1}{path}\u{1}{args}")
            }
        }
        LaunchTarget::Aumid { aumid } => {
            let aumid = aumid.trim().to_lowercase();
            if aumid.is_empty() {
                String::new()
            } else {
                format!("aumid\u{1}{aumid}")
            }
        }
        LaunchTarget::Url { url } => {
            let url = url.trim();
            if url.is_empty() {
                String::new()
            } else {
                format!("url\u{1}{}", url.to_ascii_lowercase())
            }
        }
    }
}

/// 各组按 [`AppSource::ALL`] 的顺序传入。同一键只保留先出现的条目。
///
/// 后出现的便携应用或别名不另成一条，名称并入已保留条目的备用名。
/// 别名在没有任何已有目标时不单独显示。
#[must_use]
pub fn dedupe_entries(groups: &[&[AppEntry]]) -> Vec<AppEntry> {
    let mut seen = HashMap::new();
    let mut out = Vec::new();
    for group in groups {
        for entry in *group {
            let key = launch_key(&entry.target);
            if key.is_empty() || entry.name.trim().is_empty() {
                continue;
            }
            if let Some(&index) = seen.get(&key) {
                if matches!(entry.source, AppSource::Portable | AppSource::Alias) {
                    merge_alternates(&mut out[index], entry);
                }
                continue;
            }
            if entry.source == AppSource::Alias {
                continue;
            }
            seen.insert(key, out.len());
            out.push(entry.clone());
        }
    }
    out
}

fn merge_alternates(kept: &mut AppEntry, incoming: &AppEntry) {
    push_alternate(&mut kept.alternate_names, &kept.name, &incoming.name);
    for name in &incoming.alternate_names {
        push_alternate(&mut kept.alternate_names, &kept.name, name);
    }
}

/// 与显示名不同、且名单里还没有的备用名。
pub(crate) fn push_alternate(names: &mut Vec<String>, display: &str, candidate: &str) {
    let candidate = candidate.trim();
    if candidate.is_empty() || candidate.eq_ignore_ascii_case(display.trim()) {
        return;
    }
    if names
        .iter()
        .any(|name| name.eq_ignore_ascii_case(candidate))
    {
        return;
    }
    names.push(candidate.to_owned());
}

/// 各来源上一次成功的结果。失败的来源不改这里。
#[derive(Debug, Clone, Default)]
pub struct SourceSnapshots {
    pub start_menu: Vec<AppEntry>,
    pub app_paths: Vec<AppEntry>,
    pub path: Vec<AppEntry>,
    pub store: Vec<AppEntry>,
    pub portable: Vec<AppEntry>,
    pub alias: Vec<AppEntry>,
}

impl SourceSnapshots {
    #[must_use]
    pub fn from_entries(entries: &[AppEntry]) -> Self {
        let mut snapshots = Self::default();
        for entry in entries {
            snapshots.bucket_mut(entry.source).push(entry.clone());
        }
        snapshots
    }

    pub fn replace(&mut self, source: AppSource, entries: Vec<AppEntry>) {
        *self.bucket_mut(source) = entries;
    }

    #[must_use]
    pub fn merged(&self) -> Vec<AppEntry> {
        dedupe_entries(&[
            &self.start_menu,
            &self.app_paths,
            &self.path,
            &self.store,
            &self.portable,
            &self.alias,
        ])
    }

    fn bucket_mut(&mut self, source: AppSource) -> &mut Vec<AppEntry> {
        match source {
            AppSource::StartMenu => &mut self.start_menu,
            AppSource::AppPaths => &mut self.app_paths,
            AppSource::Path => &mut self.path,
            AppSource::Store => &mut self.store,
            AppSource::Portable => &mut self.portable,
            AppSource::Alias => &mut self.alias,
        }
    }
}

/// 一次来源枚举的结果。`Err` 保留该来源原来的快照。
pub struct SourceAttempt {
    pub source: AppSource,
    pub result: Result<Vec<AppEntry>, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceError {
    pub source: AppSource,
    pub message: String,
}

pub struct ApplyOutcome {
    pub entries: Vec<AppEntry>,
    pub errors: Vec<SourceError>,
}

/// 把一轮枚举写进快照。失败的来源不动，其它来源替换。
pub fn apply_source_results(
    snapshots: &mut SourceSnapshots,
    attempts: &[SourceAttempt],
) -> ApplyOutcome {
    let mut errors = Vec::new();
    for attempt in attempts {
        match &attempt.result {
            Ok(entries) => snapshots.replace(attempt.source, entries.clone()),
            Err(message) => errors.push(SourceError {
                source: attempt.source,
                message: message.clone(),
            }),
        }
    }
    ApplyOutcome {
        entries: snapshots.merged(),
        errors,
    }
}

pub(crate) fn normalize_path_key(path: &Path) -> String {
    let text = path.to_string_lossy();
    let slashed = text.trim().replace('/', "\\");
    let stripped = strip_verbatim(&slashed);
    let logical = lexical_normalize(&stripped);
    trim_trailing_slash(&logical).to_lowercase()
}

fn strip_verbatim(text: &str) -> String {
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        rest.to_owned()
    } else {
        text.to_owned()
    }
}

fn lexical_normalize(text: &str) -> String {
    let mut out = PathBuf::new();
    for component in Path::new(text).components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out.to_string_lossy().into_owned()
}

fn trim_trailing_slash(text: &str) -> &str {
    if text.len() > 3 && text.ends_with('\\') {
        text.trim_end_matches('\\')
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path_target(path: &str, args: &str, work: Option<&str>) -> LaunchTarget {
        LaunchTarget::Path {
            path: PathBuf::from(path),
            args: args.to_owned(),
            working_directory: work.map(PathBuf::from),
        }
    }

    fn entry(name: &str, source: AppSource, target: LaunchTarget) -> AppEntry {
        AppEntry {
            name: name.to_owned(),
            source,
            target,
            icon_path: None,
            icon_index: 0,
            alternate_names: Vec::new(),
        }
    }

    #[test]
    fn source_order_is_system_sources_then_user_catalog() {
        assert_eq!(
            AppSource::ALL,
            [
                AppSource::StartMenu,
                AppSource::AppPaths,
                AppSource::Path,
                AppSource::Store,
                AppSource::Portable,
                AppSource::Alias,
            ]
        );
    }

    #[test]
    fn path_key_folds_case_slashes_and_verbatim_prefix() {
        let left = path_target(r"C:\Windows\System32\notepad.exe", "", None);
        let right = path_target(r"\\?\c:/windows/system32/notepad.exe", "  ", None);
        assert_eq!(launch_key(&left), launch_key(&right));
        assert!(!launch_key(&left).is_empty());
    }

    #[test]
    fn args_stay_in_the_key_and_working_directory_does_not() {
        let plain = path_target(r"C:\App\tool.exe", "", Some(r"C:\App"));
        let other_dir = path_target(r"C:\App\tool.exe", "", Some(r"D:\Work"));
        let with_args = path_target(r"C:\App\tool.exe", "--profile work", Some(r"C:\App"));
        assert_eq!(launch_key(&plain), launch_key(&other_dir));
        assert_ne!(launch_key(&plain), launch_key(&with_args));
        let padded = path_target(r"C:\App\tool.exe", "  --profile work  ", None);
        assert_eq!(launch_key(&with_args), launch_key(&padded));
    }

    #[test]
    fn aumid_key_is_case_insensitive() {
        let left = LaunchTarget::Aumid {
            aumid: "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into(),
        };
        let right = LaunchTarget::Aumid {
            aumid: "  microsoft.windowscalculator_8wekyb3d8bbwe!app ".into(),
        };
        assert_eq!(launch_key(&left), launch_key(&right));
        assert_ne!(
            launch_key(&left),
            launch_key(&path_target(r"C:\calc.exe", "", None))
        );
    }

    #[test]
    fn same_target_in_start_menu_and_app_paths_is_kept_once() {
        let start = entry(
            "Windows PowerShell",
            AppSource::StartMenu,
            path_target(
                r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
                "",
                None,
            ),
        );
        let paths = entry(
            "powershell",
            AppSource::AppPaths,
            path_target(
                r"c:\windows\system32\windowspowershell\v1.0\powershell.exe",
                "",
                None,
            ),
        );
        let merged = dedupe_entries(&[std::slice::from_ref(&start), std::slice::from_ref(&paths)]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].name, "Windows PowerShell");
        assert_eq!(merged[0].source, AppSource::StartMenu);
    }

    #[test]
    fn shortcut_args_keep_a_separate_entry() {
        let plain = entry(
            "Tool",
            AppSource::AppPaths,
            path_target(r"C:\App\tool.exe", "", None),
        );
        let with_args = entry(
            "Tool profile",
            AppSource::StartMenu,
            path_target(r"C:\App\tool.exe", "--kept", None),
        );
        let merged = dedupe_entries(&[
            std::slice::from_ref(&with_args),
            std::slice::from_ref(&plain),
        ]);
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].target, with_args.target);
    }

    #[test]
    fn failed_source_keeps_previous_and_other_sources_replace() {
        let previous_start = entry(
            "Kept",
            AppSource::StartMenu,
            path_target(r"C:\Kept\app.exe", "", None),
        );
        let previous_path = entry(
            "OldPath",
            AppSource::Path,
            path_target(r"C:\Old\old.exe", "", None),
        );
        let fresh_path = entry(
            "NewPath",
            AppSource::Path,
            path_target(r"C:\New\new.exe", "", None),
        );
        let mut snapshots = SourceSnapshots::from_entries(&[previous_start.clone(), previous_path]);
        let outcome = apply_source_results(
            &mut snapshots,
            &[
                SourceAttempt {
                    source: AppSource::StartMenu,
                    result: Err("开始菜单读失败".into()),
                },
                SourceAttempt {
                    source: AppSource::Path,
                    result: Ok(vec![fresh_path.clone()]),
                },
            ],
        );
        assert_eq!(outcome.errors.len(), 1);
        assert_eq!(outcome.errors[0].source, AppSource::StartMenu);
        assert!(outcome.entries.iter().any(|entry| entry.name == "Kept"));
        assert!(outcome.entries.iter().any(|entry| entry.name == "NewPath"));
        assert!(!outcome.entries.iter().any(|entry| entry.name == "OldPath"));
        assert!(
            snapshots
                .start_menu
                .iter()
                .any(|entry| entry.name == "Kept")
        );
    }

    #[test]
    fn url_key_folds_ascii_case_and_keeps_the_rest() {
        let left = LaunchTarget::Url {
            url: "steam://rungameid/570".into(),
        };
        let right = LaunchTarget::Url {
            url: "  STEAM://rungameid/570 ".into(),
        };
        assert_eq!(launch_key(&left), launch_key(&right));
        assert_ne!(
            launch_key(&left),
            launch_key(&LaunchTarget::Url {
                url: "steam://rungameid/730".into(),
            })
        );
    }

    #[test]
    fn portable_duplicate_keeps_the_system_name_and_adds_an_alternate() {
        let start = entry(
            "记事本",
            AppSource::StartMenu,
            path_target(r"C:\Windows\notepad.exe", "", None),
        );
        let portable = entry(
            "便携记事本",
            AppSource::Portable,
            path_target(r"c:\windows\notepad.exe", "", None),
        );
        let merged = dedupe_entries(&[
            std::slice::from_ref(&start),
            std::slice::from_ref(&portable),
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].name, "记事本");
        assert_eq!(merged[0].source, AppSource::StartMenu);
        assert_eq!(merged[0].alternate_names, vec!["便携记事本".to_owned()]);
    }

    #[test]
    fn unique_portable_is_kept_and_orphan_alias_is_not() {
        let portable = entry(
            "便携工具",
            AppSource::Portable,
            path_target(r"D:\Tools\tool.exe", "", None),
        );
        let orphan = entry(
            "找不到",
            AppSource::Alias,
            path_target(r"D:\Missing\missing.exe", "", None),
        );
        let attached = entry(
            "工具别名",
            AppSource::Alias,
            path_target(r"D:\Tools\tool.exe", "", None),
        );
        let merged = dedupe_entries(&[
            std::slice::from_ref(&portable),
            std::slice::from_ref(&orphan),
            std::slice::from_ref(&attached),
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].name, "便携工具");
        assert_eq!(merged[0].alternate_names, vec!["工具别名".to_owned()]);
    }
}
