//! 进程采样和延迟原始记录汇总。
//!
//! 采样列按 architecture.md「性能测量」。延迟记录的 JSONL 是给 #20、#21 写、给本工具读的交换格式，
//! 不是产品数据。P95 取升序第 ⌈0.95 × N⌉ 个。有效样本不足 100 时结果标明「不足」。

mod latency;
mod names;
mod session;

#[cfg(windows)]
mod win;

pub use latency::{
    LatencyGroup, LatencyReport, MIN_VALID_SAMPLES, P95_RULE, RawLatency, parse_latency_jsonl,
    summarize,
};
pub use names::{
    context_switch_counter, counter_process_name, exe_names_match, id_process_counter,
    select_one_pid,
};
pub use session::{
    CSV_HEADER, Clock, ManualClock, Probe, Reading, Sample, StopReason, SystemClock, format_utc,
    p95, p95_index_1based, sample_to_writer,
};

use std::path::{Path, PathBuf};
use std::time::Duration;

use std::fmt;

#[derive(Debug)]
pub struct ToolError {
    message: String,
}

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ToolError {}

impl From<std::io::Error> for ToolError {
    fn from(err: std::io::Error) -> Self {
        Self::new(err.to_string())
    }
}

impl From<serde_json::Error> for ToolError {
    fn from(err: serde_json::Error) -> Self {
        Self::new(err.to_string())
    }
}

#[derive(Debug, Clone)]
pub enum SampleTarget {
    Pid(u32),
    Name(String),
}

#[derive(Debug, Clone)]
pub struct SampleRequest {
    pub target: SampleTarget,
    pub duration: Duration,
    pub interval: Duration,
    pub out: PathBuf,
    pub user_profile: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct SampleRun {
    pub samples: usize,
    pub stop: StopReason,
    pub pid: u32,
    pub counter: String,
    pub out: PathBuf,
}

pub fn run_sample(request: &SampleRequest) -> Result<SampleRun, ToolError> {
    refuse_output_path(&request.out, request.user_profile.as_deref())?;
    #[cfg(windows)]
    {
        win::run_sample(request)
    }
    #[cfg(not(windows))]
    {
        Err(ToolError::new("采样只能在 Windows 上运行"))
    }
}

pub fn refuse_output_path(path: &Path, user_profile: Option<&Path>) -> Result<(), ToolError> {
    let Some(profile) = user_profile else {
        return Ok(());
    };
    let path = normalize_path(&absolute_path(path));
    let lanwork = normalize_path(&profile.join("Documents").join("Lanwork"));
    let maydolist = normalize_path(&profile.join("Documents").join("MayDolist"));
    if path_within(&path, &lanwork) || path_within(&path, &maydolist) {
        return Err(ToolError::new(format!(
            "拒绝写入 {}。采样结果不能放进 Documents\\Lanwork 或 Documents\\MayDolist",
            path.display()
        )));
    }
    Ok(())
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy().replace('/', "\\");
    let stripped = text.strip_prefix(r"\\?\").unwrap_or(&text);
    PathBuf::from(stripped.to_ascii_lowercase())
}

fn path_within(path: &Path, root: &Path) -> bool {
    let path = path.to_string_lossy();
    let root = root.to_string_lossy();
    let path = path.trim_end_matches('\\');
    let root = root.trim_end_matches('\\');
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('\\'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampler_does_not_write_into_user_documents() {
        let mut profile = if cfg!(windows) {
            PathBuf::from(r"C:\")
        } else {
            PathBuf::from("/")
        };
        profile.push("lanwork-sample-guard");
        profile.push("Profile");
        let lanwork = refuse_output_path(
            &profile.join("Documents").join("Lanwork").join("a.csv"),
            Some(&profile),
        );
        assert!(lanwork.unwrap_err().to_string().contains("拒绝写入"));
        let maydolist = refuse_output_path(
            &profile.join("documents").join("maydolist").join("a.csv"),
            Some(&profile),
        );
        assert!(maydolist.unwrap_err().to_string().contains("拒绝写入"));
        let mut outside = if cfg!(windows) {
            PathBuf::from(r"C:\")
        } else {
            PathBuf::from("/")
        };
        outside.push("lanwork-sample-guard");
        outside.push("out.csv");
        assert!(refuse_output_path(&outside, Some(&profile)).is_ok());
    }

    #[test]
    fn windows_sampler_source_does_not_trim_the_working_set() {
        let source = include_str!("win.rs").to_ascii_lowercase();
        assert!(!source.contains("emptyworkingset("));
        assert!(!source.contains("setprocessworkingsetsize("));
    }
}
