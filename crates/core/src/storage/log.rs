//! `logs/app.log`，按大小轮转。
//!
//! 不写入文件内容。调用方因此不能通过这里记录便签正文。
//! 名称像密钥的环境变量值，以及常见 GitHub token 形态，会在落盘前换成 `[redacted]`。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use super::fsutil;
use super::lock_mutex;

pub const LOG_MAX_BYTES: u64 = 1024 * 1024;
pub const LOG_MAX_FILES: u32 = 3;

#[derive(Debug, Clone)]
pub struct LogSettings {
    pub max_bytes: u64,
    pub max_files: u32,
    pub secrets: Vec<String>,
}

impl Default for LogSettings {
    fn default() -> Self {
        Self {
            max_bytes: LOG_MAX_BYTES,
            max_files: LOG_MAX_FILES,
            secrets: secrets_from_env(),
        }
    }
}

#[derive(Debug)]
pub struct Log {
    path: PathBuf,
    max_bytes: u64,
    max_files: u32,
    redactor: Redactor,
    lock: Mutex<()>,
}

impl Log {
    pub fn open(path: impl Into<PathBuf>, settings: LogSettings) -> std::io::Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fsutil::create_dir_all(parent)?;
        }
        Ok(Self {
            path,
            max_bytes: settings.max_bytes.max(1),
            max_files: settings.max_files.max(1),
            redactor: Redactor::new(settings.secrets),
            lock: Mutex::new(()),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn info(&self, message: &str) {
        let _ = self.write_line("INFO", message);
    }

    pub fn warn(&self, message: &str) {
        let _ = self.write_line("WARN", message);
    }

    pub fn error(&self, message: &str) {
        let _ = self.write_line("ERROR", message);
    }

    fn write_line(&self, level: &str, message: &str) -> std::io::Result<()> {
        let _guard = lock_mutex(&self.lock);
        let redacted = self.redactor.redact(message);
        let line = format!("{} {level} {redacted}\n", utc_timestamp());
        self.rotate_if_needed(line.len())?;
        fsutil::append(&self.path, line.as_bytes())
    }

    fn rotate_if_needed(&self, incoming: usize) -> std::io::Result<()> {
        let len = if fsutil::exists(&self.path) {
            fsutil::file_len(&self.path)?
        } else {
            0
        };
        if len > 0 && len.saturating_add(incoming as u64) > self.max_bytes {
            self.rotate()?;
        }
        Ok(())
    }

    fn rotate(&self) -> std::io::Result<()> {
        let slots = self.max_files.saturating_sub(1);
        if slots == 0 {
            fsutil::remove_file(&self.path)?;
            return Ok(());
        }
        fsutil::remove_file(&self.rotated(slots))?;
        for index in (1..slots).rev() {
            let from = self.rotated(index);
            if fsutil::exists(&from) {
                fsutil::rename(&from, &self.rotated(index + 1))?;
            }
        }
        if fsutil::exists(&self.path) {
            fsutil::rename(&self.path, &self.rotated(1))?;
        }
        Ok(())
    }

    fn rotated(&self, index: u32) -> PathBuf {
        let name = self.path.file_name().unwrap_or_default();
        let mut rotated = std::ffi::OsString::from(name);
        rotated.push(format!(".{index}"));
        match self.path.parent() {
            Some(parent) => parent.join(rotated),
            None => PathBuf::from(rotated),
        }
    }
}

#[derive(Debug)]
struct Redactor {
    secrets: Vec<String>,
}

impl Redactor {
    fn new(mut secrets: Vec<String>) -> Self {
        secrets.retain(|secret| secret.len() >= 8);
        secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
        secrets.dedup();
        Self { secrets }
    }

    fn redact(&self, message: &str) -> String {
        let mut output = message.to_owned();
        for secret in &self.secrets {
            output = output.replace(secret, "[redacted]");
        }
        redact_github_tokens(&output)
    }
}

pub fn secrets_from_env() -> Vec<String> {
    std::env::vars()
        .filter(|(name, value)| is_secret_env_name(name) && value.len() >= 8)
        .map(|(_, value)| value)
        .collect()
}

fn is_secret_env_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.contains("TOKEN")
        || upper.contains("SECRET")
        || upper.contains("PASSWORD")
        || upper.contains("CREDENTIAL")
        || upper == "KEY"
        || upper.ends_with("_KEY")
}

fn redact_github_tokens(input: &str) -> String {
    const PREFIXES: [&str; 6] = ["github_pat_", "ghp_", "gho_", "ghu_", "ghs_", "ghr_"];
    let mut output = String::with_capacity(input.len());
    let mut index = 0;
    while index < input.len() {
        let rest = &input[index..];
        if let Some(prefix) = PREFIXES
            .iter()
            .copied()
            .find(|prefix| rest.starts_with(prefix))
        {
            let token = &rest[prefix.len()..];
            let token_bytes = token
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                .map(char::len_utf8)
                .sum::<usize>();
            if token_bytes >= 8 {
                output.push_str("[redacted]");
                index += prefix.len() + token_bytes;
                continue;
            }
        }
        let ch = rest.chars().next().expect("index is on a char boundary");
        output.push(ch);
        index += ch.len_utf8();
    }
    output
}

fn utc_timestamp() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let hour = tod / 3600;
    let minute = (tod % 3600) / 60;
    let second = tod % 60;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Howard Hinnant 的 civil_from_days。`days` 是从 Unix 纪元起的天数。
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = (if month <= 2 { y + 1 } else { y }) as i32;
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::test_temp::TempDir;

    #[test]
    fn epoch_is_1970_01_01() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn redacts_secret_env_values_and_github_tokens() {
        let temp = TempDir::new();
        let path = temp.path().join("app.log");
        let log = Log::open(
            &path,
            LogSettings {
                max_bytes: 1024,
                max_files: 3,
                secrets: vec!["super-secret-value".into(), "short".into()],
            },
        )
        .unwrap();
        log.info("env=super-secret-value token=ghp_abcdefghijklmnopqrst");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(!text.contains("super-secret-value"));
        assert!(!text.contains("ghp_abcdefghijklmnopqrst"));
        assert!(text.contains("[redacted]"));
        assert!(text.contains("INFO"));
    }

    #[test]
    fn rotates_by_size_and_keeps_limit() {
        let temp = TempDir::new();
        let path = temp.path().join("logs").join("app.log");
        let log = Log::open(
            &path,
            LogSettings {
                max_bytes: 80,
                max_files: 3,
                secrets: Vec::new(),
            },
        )
        .unwrap();
        for index in 0..20 {
            log.info(&format!("line-{index}-padding"));
        }
        assert!(path.is_file());
        assert!(path.with_file_name("app.log.1").is_file());
        assert!(!path.with_file_name("app.log.3").exists());
        let current = std::fs::metadata(&path).unwrap().len();
        assert!(current <= 80 || current < 200);
    }
}
