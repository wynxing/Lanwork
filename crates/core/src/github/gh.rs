//! 本机 `gh` 的探测和调用。
//!
//! 生产路径是 [`ProcessGh`]，只启动 `gh`，不经过 shell。Windows 上带 [`CREATE_NO_WINDOW`]。
//! 不读取、不保存、不记录环境变量。标准错误只用于归类，不写日志。
//! 测试用 [`CommandRunner`] 替换进程，或直接实现 [`GhClient`]。

use std::sync::Arc;

use super::error::GhCallError;
use super::parse::{FetchedRepo, parse_issues, parse_pulls};

/// `CreateProcess` 的 `CREATE_NO_WINDOW`。只在 Windows 上传给子进程。
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 单次 `gh pr list` / `gh issue list` 的 `--limit`。
///
/// 产品规格没有写分页。超过这个数量的条目不会进入这次快照。
pub const GH_LIST_LIMIT: &str = "200";

const PR_JSON_FIELDS: &str = "number,title,url,state,isDraft,updatedAt,mergedAt";
const ISSUE_JSON_FIELDS: &str = "number,title,url,state,updatedAt";

/// 一次无窗口进程的结果。`stdout` / `stderr` 留在内存里供归类，不进入日志。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandCapture {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnFailure {
    NotFound,
    Failed,
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, args: &[&str]) -> Result<CommandCapture, SpawnFailure>;
}

/// 探测结果。登录名不返回，避免被记进日志。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhProbe {
    Ready { version: String },
    NotInstalled,
    NotLoggedIn { version: String },
}

pub trait GhClient: Send + Sync {
    fn probe(&self) -> Result<GhProbe, GhCallError>;
    fn fetch(&self, repo: &str) -> Result<FetchedRepo, GhCallError>;
}

/// 调用本机 `gh`。
pub struct ProcessGh {
    runner: Arc<dyn CommandRunner>,
}

impl ProcessGh {
    #[must_use]
    pub fn system() -> Self {
        Self {
            runner: Arc::new(SystemRunner),
        }
    }

    #[must_use]
    pub fn with_runner(runner: Arc<dyn CommandRunner>) -> Self {
        Self { runner }
    }
}

impl GhClient for ProcessGh {
    fn probe(&self) -> Result<GhProbe, GhCallError> {
        let version = match self.runner.run(&["version"]) {
            Err(SpawnFailure::NotFound) => return Ok(GhProbe::NotInstalled),
            Err(SpawnFailure::Failed) => return Err(GhCallError::Command),
            Ok(capture) if capture.code != 0 => {
                return Err(classify_output(&capture));
            }
            Ok(capture) => version_from_stdout(&capture.stdout),
        };
        match self.runner.run(&["auth", "status"]) {
            Err(SpawnFailure::NotFound) => Ok(GhProbe::NotInstalled),
            Err(SpawnFailure::Failed) => Err(GhCallError::Command),
            Ok(capture) if capture.code == 0 => Ok(GhProbe::Ready { version }),
            Ok(capture) => {
                let err = classify_output(&capture);
                match err {
                    GhCallError::Network | GhCallError::RateLimit => Err(err),
                    _ => Ok(GhProbe::NotLoggedIn { version }),
                }
            }
        }
    }

    fn fetch(&self, repo: &str) -> Result<FetchedRepo, GhCallError> {
        if !repo_arg_ok(repo) {
            return Err(GhCallError::InvalidRepo);
        }
        let pulls = self.output(&pr_args(repo))?;
        let issues = self.output(&issue_args(repo))?;
        Ok(FetchedRepo {
            pulls: parse_pulls(&pulls)?,
            issues: parse_issues(&issues)?,
        })
    }
}

impl ProcessGh {
    fn output(&self, args: &[&str]) -> Result<Vec<u8>, GhCallError> {
        match self.runner.run(args) {
            Err(SpawnFailure::NotFound) => Err(GhCallError::NotInstalled),
            Err(SpawnFailure::Failed) => Err(GhCallError::Command),
            Ok(capture) if capture.code != 0 => Err(classify_output(&capture)),
            Ok(capture) => Ok(capture.stdout),
        }
    }
}

struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, args: &[&str]) -> Result<CommandCapture, SpawnFailure> {
        let mut command = std::process::Command::new("gh");
        command.args(args);
        command.stdin(std::process::Stdio::null());
        command.stdout(std::process::Stdio::piped());
        command.stderr(std::process::Stdio::piped());
        apply_no_window(&mut command);
        match command.output() {
            Ok(output) => Ok(CommandCapture {
                code: output.status.code().unwrap_or(1),
                stdout: output.stdout,
                stderr: output.stderr,
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(SpawnFailure::NotFound),
            Err(_) => Err(SpawnFailure::Failed),
        }
    }
}

fn apply_no_window(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

pub(crate) fn pr_args(repo: &str) -> Vec<&str> {
    vec![
        "pr",
        "list",
        "--repo",
        repo,
        "--state",
        "all",
        "--limit",
        GH_LIST_LIMIT,
        "--json",
        PR_JSON_FIELDS,
    ]
}

pub(crate) fn issue_args(repo: &str) -> Vec<&str> {
    vec![
        "issue",
        "list",
        "--repo",
        repo,
        "--state",
        "all",
        "--limit",
        GH_LIST_LIMIT,
        "--json",
        ISSUE_JSON_FIELDS,
    ]
}

fn repo_arg_ok(repo: &str) -> bool {
    !repo.is_empty() && !repo.starts_with('-') && !repo.chars().any(char::is_control)
}

fn classify_output(capture: &CommandCapture) -> GhCallError {
    let stdout = String::from_utf8_lossy(&capture.stdout);
    let stderr = String::from_utf8_lossy(&capture.stderr);
    classify_text(&stdout, &stderr)
}

fn classify_text(stdout: &str, stderr: &str) -> GhCallError {
    let mut combined = String::with_capacity(stdout.len() + stderr.len() + 1);
    combined.push_str(stdout);
    combined.push('\n');
    combined.push_str(stderr);
    let lower = combined.to_ascii_lowercase();
    if lower.contains("rate limit") {
        return GhCallError::RateLimit;
    }
    if lower.contains("not logged") || lower.contains("auth login") {
        return GhCallError::NotLoggedIn;
    }
    if is_network(&lower) {
        return GhCallError::Network;
    }
    if lower.contains("could not resolve to a repository")
        || lower.contains("http 404")
        || lower.contains("not found")
    {
        return GhCallError::NotFound;
    }
    GhCallError::Command
}

fn is_network(lower: &str) -> bool {
    [
        "error connecting",
        "connection refused",
        "timed out",
        "timeout",
        "no such host",
        "tls",
        "network",
        "dial tcp",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// 从 `gh version` 的输出取出版本号。无法识别时返回 `unknown`，不回传原文。
#[must_use]
pub fn version_from_stdout(stdout: &[u8]) -> String {
    let text = String::from_utf8_lossy(stdout);
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("gh version ") {
            return sanitize_version(rest.split_whitespace().next().unwrap_or(""));
        }
    }
    "unknown".to_owned()
}

fn sanitize_version(token: &str) -> String {
    if token.len() > 40 || !token.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        return "unknown".to_owned();
    }
    if token
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '+' | '-'))
    {
        token.to_owned()
    } else {
        "unknown".to_owned()
    }
}

/// 日志里只允许这一行形式。`version` 必须已经过 [`version_from_stdout`]。
#[must_use]
pub fn version_log_line(version: &str) -> String {
    format!("gh version {}", sanitize_version(version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct Script {
        calls: Mutex<Vec<Vec<String>>>,
        responses: Vec<Result<CommandCapture, SpawnFailure>>,
    }

    impl Script {
        fn new(responses: Vec<Result<CommandCapture, SpawnFailure>>) -> Arc<Self> {
            Arc::new(Self {
                calls: Mutex::new(Vec::new()),
                responses,
            })
        }
    }

    impl CommandRunner for Script {
        fn run(&self, args: &[&str]) -> Result<CommandCapture, SpawnFailure> {
            let mut calls = self.calls.lock().unwrap_or_else(|err| err.into_inner());
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
    fn create_no_window_matches_win32() {
        assert_eq!(CREATE_NO_WINDOW, 0x0800_0000);
    }

    #[test]
    fn version_log_drops_raw_text() {
        assert_eq!(
            version_from_stdout(b"gh version 2.62.0 (2024-10-21)\nhttps://example.invalid\n"),
            "2.62.0"
        );
        assert_eq!(
            version_from_stdout(b"gh version ghp_abcdefghijklmnopqrst\n"),
            "unknown"
        );
        assert_eq!(
            version_log_line("2.62.0\nGH_TOKEN=secret"),
            "gh version unknown"
        );
    }

    #[test]
    fn classifies_recorded_failures_without_keeping_bodies() {
        assert_eq!(
            classify_text("", "HTTP 403: API rate limit exceeded"),
            GhCallError::RateLimit
        );
        assert_eq!(
            classify_text(
                "",
                "You are not logged into any GitHub hosts. Run gh auth login"
            ),
            GhCallError::NotLoggedIn
        );
        assert_eq!(
            classify_text("", "error connecting to api.github.com"),
            GhCallError::Network
        );
        assert_eq!(
            classify_text("", "GraphQL: Could not resolve to a Repository"),
            GhCallError::NotFound
        );
        assert_eq!(
            classify_text("", "HTTP 404: Not Found"),
            GhCallError::NotFound
        );
        assert_eq!(classify_text("nope", ""), GhCallError::Command);
    }

    #[test]
    fn probe_and_fetch_use_argv_and_skip_unsafe_repos() {
        let script = Script::new(vec![
            Ok(capture(
                0,
                "gh version 2.62.0 (2024-10-21)\n",
                "ghp_abcdefghijklmnopqrst STDERR-MARKER",
            )),
            Ok(capture(0, "", "")),
            Ok(capture(
                0,
                r#"[{"number":1,"title":"A","url":"https://example.com/a","state":"OPEN"}]"#,
                "",
            )),
            Ok(capture(0, "[]", "")),
        ]);
        let gh = ProcessGh::with_runner(script.clone());
        let probe = gh.probe().unwrap();
        assert_eq!(
            probe,
            GhProbe::Ready {
                version: "2.62.0".into()
            }
        );
        let fetched = gh.fetch("example/widget").unwrap();
        assert_eq!(fetched.pulls.len(), 1);
        assert!(fetched.issues.is_empty());
        let calls = script.calls.lock().unwrap();
        assert_eq!(calls[0], vec!["version".to_owned()]);
        assert_eq!(calls[1], vec!["auth".to_owned(), "status".to_owned()]);
        assert_eq!(calls[2][0], "pr");
        assert_eq!(calls[2][3], "example/widget");
        assert!(calls[2].iter().any(|arg| arg == GH_LIST_LIMIT));
        assert_eq!(calls[3][0], "issue");
        drop(calls);
        assert_eq!(
            gh.fetch("-example/widget").unwrap_err(),
            GhCallError::InvalidRepo
        );
        assert_eq!(script.calls.lock().unwrap().len(), 4);
    }

    #[test]
    fn missing_binary_is_not_installed() {
        let script = Script::new(vec![Err(SpawnFailure::NotFound)]);
        let gh = ProcessGh::with_runner(script);
        assert_eq!(gh.probe().unwrap(), GhProbe::NotInstalled);
    }

    #[test]
    fn auth_failure_without_network_is_logged_out() {
        let script = Script::new(vec![
            Ok(capture(0, "gh version 2.62.0\n", "")),
            Ok(capture(1, "", "You are not logged into any GitHub hosts")),
        ]);
        let gh = ProcessGh::with_runner(script);
        assert_eq!(
            gh.probe().unwrap(),
            GhProbe::NotLoggedIn {
                version: "2.62.0".into()
            }
        );
    }
}
