//! 安装包核对、安装版和便携版、以及退出后重新启动的脚本。
//!
//! 脚本只启动安装程序和原来的 `lanwork.exe`。参数里没有数据目录，也没有 `/DIR`。

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::fetch::{MAX_INSTALLER_BYTES, installer_file_name};
use super::version::Version;
use super::view::{TEXT_DOWNLOAD_FAILED, TEXT_VERIFY_FAILED};

/// 静默更新传给 Inno Setup 的参数。没有安装目录，也没有数据目录。
pub const INSTALL_ARGS: [&str; 4] = [
    "/VERYSILENT",
    "/SUPPRESSMSGBOXES",
    "/NORESTART",
    "/CLOSEAPPLICATIONS",
];

/// 边下载边算 SHA-256。
#[derive(Clone)]
pub struct ContentHash(Sha256);

impl ContentHash {
    #[must_use]
    pub fn new() -> Self {
        Self(Sha256::new())
    }

    pub fn update(&mut self, data: &[u8]) {
        Digest::update(&mut self.0, data);
    }

    #[must_use]
    pub fn finish_hex(self) -> String {
        hex(&self.0.finalize())
    }
}

impl Default for ContentHash {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    let mut hash = ContentHash::new();
    hash.update(data);
    hash.finish_hex()
}

#[must_use]
pub fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[must_use]
pub fn sha256_eq(left: &str, right: &str) -> bool {
    is_sha256_hex(left) && is_sha256_hex(right) && left.eq_ignore_ascii_case(right)
}

/// 下载完的安装包能不能启动。空文件、超限、哈希不一致都不行。
pub fn review_download(expected: &str, actual: &str, bytes: u64) -> Result<(), &'static str> {
    if bytes == 0 || bytes > MAX_INSTALLER_BYTES {
        return Err(TEXT_DOWNLOAD_FAILED);
    }
    if !sha256_eq(expected, actual) {
        return Err(TEXT_VERIFY_FAILED);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edition {
    Installed,
    Portable,
}

/// 安装目录里有 Inno Setup 的 `unins000.exe` 就是安装版。便携版的 zip 里没有这个文件。
#[must_use]
pub fn edition_of(exe_dir: &Path) -> Edition {
    if exe_dir.join("unins000.exe").is_file() {
        Edition::Installed
    } else {
        Edition::Portable
    }
}

/// 安装包要下到临时目录。落在数据目录或缓存目录里就拒绝。
#[must_use]
pub fn download_path(
    temp_dir: &Path,
    data_dir: &Path,
    cache_dir: &Path,
    version: Version,
) -> Option<PathBuf> {
    let path = temp_dir.join(installer_file_name(version));
    if path.starts_with(data_dir) || path.starts_with(cache_dir) {
        return None;
    }
    Some(path)
}

/// 安装失败时写退出码的文件。优先放在缓存目录的上一级（`%LOCALAPPDATA%\Lanwork`），
/// 不放进数据目录。那里也不行时改放临时目录。两处都落在数据目录里就返回 `None`。
#[must_use]
pub fn install_failure_marker(
    cache_dir: &Path,
    data_dir: &Path,
    temp_dir: &Path,
) -> Option<PathBuf> {
    let beside_cache = cache_dir
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| parent.join("update-failed.txt"));
    if let Some(marker) = beside_cache
        && !marker.starts_with(data_dir)
    {
        return Some(marker);
    }
    let marker = temp_dir.join("Lanwork-update-failed.txt");
    if marker.starts_with(data_dir) || marker.starts_with(cache_dir) {
        return None;
    }
    Some(marker)
}

/// 下次启动读到失败标记时写进日志的那一行。只接受十进制退出码，其余内容不写进日志。
#[must_use]
pub fn install_failure_log(marker_text: &str) -> String {
    let code = marker_text.trim();
    if !code.is_empty() && code.len() <= 10 && code.bytes().all(|byte| byte.is_ascii_digit()) {
        format!("安装失败：安装程序退出码 {code}")
    } else {
        "安装失败".to_owned()
    }
}

/// 本进程退出之后运行的 PowerShell。等进程结束，再静默安装，然后启动原来的程序。
///
/// 安装程序的退出码不是 0，或者启动失败时，把退出码写到 `marker`。路径里有控制字符时返回 `None`。
#[must_use]
pub fn update_helper_script(
    pid: u32,
    installer: &Path,
    app: &Path,
    marker: &Path,
) -> Option<String> {
    let installer = ps_quote(&path_text(installer)?)?;
    let app = ps_quote(&path_text(app)?)?;
    let marker = ps_quote(&path_text(marker)?)?;
    let args = INSTALL_ARGS
        .iter()
        .map(|arg| format!("'{arg}'"))
        .collect::<Vec<_>>()
        .join(",");
    Some(format!(
        "Wait-Process -Id {pid} -ErrorAction SilentlyContinue; Start-Sleep -Seconds 1; $code = 1; $failed = $false; try {{ $proc = Start-Process -FilePath {installer} -ArgumentList {args} -Wait -PassThru -ErrorAction Stop; if ($null -eq $proc.ExitCode -or $proc.ExitCode -ne 0) {{ $failed = $true; if ($null -ne $proc.ExitCode) {{ $code = $proc.ExitCode }} }} }} catch {{ $failed = $true; $code = 1; Write-Error -ErrorAction Continue $_.Exception.Message }}; if ($failed) {{ Set-Content -LiteralPath {marker} -Value ([string]$code) -Encoding ascii -ErrorAction Continue }}; if (Test-Path -LiteralPath {app}) {{ Start-Process -FilePath {app} }}"
    ))
}

fn path_text(path: &Path) -> Option<String> {
    let text = path.to_str()?.to_owned();
    if text.is_empty() || text.chars().any(|ch| ch.is_control()) {
        return None;
    }
    Some(text)
}

fn ps_quote(text: &str) -> Option<String> {
    if text.chars().any(|ch| ch.is_control()) {
        return None;
    }
    Some(format!("'{}'", text.replace('\'', "''")))
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    #[test]
    fn hash_matches_only_the_same_bytes() {
        let bytes = b"lanwork-setup";
        let digest = sha256_hex(bytes);
        assert!(is_sha256_hex(&digest));
        assert!(sha256_eq(&digest, &digest.to_ascii_uppercase()));
        assert!(review_download(&digest, &digest, bytes.len() as u64).is_ok());
        let mut tampered = digest.clone();
        tampered.replace_range(0..1, "0");
        if tampered == digest {
            tampered.replace_range(0..1, "1");
        }
        assert_eq!(
            review_download(&digest, &tampered, bytes.len() as u64),
            Err(TEXT_VERIFY_FAILED)
        );
        assert_eq!(
            review_download(&digest, &digest, 0),
            Err(TEXT_DOWNLOAD_FAILED)
        );
        assert_eq!(
            review_download(&digest, &digest, MAX_INSTALLER_BYTES + 1),
            Err(TEXT_DOWNLOAD_FAILED)
        );
    }

    #[test]
    fn streaming_hash_matches_one_shot() {
        let bytes = b"chunked-installer-bytes";
        let mut hash = ContentHash::new();
        hash.update(&bytes[..4]);
        hash.update(&bytes[4..]);
        assert_eq!(hash.finish_hex(), sha256_hex(bytes));
    }

    #[test]
    fn unins000_marks_the_installed_edition() {
        let dir = temp_dir();
        assert_eq!(edition_of(&dir), Edition::Portable);
        fs::write(dir.join("unins000.exe"), b"un").unwrap();
        assert_eq!(edition_of(&dir), Edition::Installed);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_path_stays_out_of_the_data_and_cache_directories() {
        let version = Version::new(0, 3, 0);
        let temp = Path::new(r"C:\Users\me\AppData\Local\Temp");
        let path = download_path(
            temp,
            Path::new(r"C:\Users\me\Documents\Lanwork"),
            Path::new(r"C:\Users\me\AppData\Local\Lanwork\cache"),
            version,
        )
        .unwrap();
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some("Lanwork-Setup-0.3.0.exe")
        );
        assert_eq!(path.parent(), Some(temp));
        assert!(
            download_path(
                Path::new(r"C:\Users\me\Documents\Lanwork"),
                Path::new(r"C:\Users\me\Documents\Lanwork"),
                Path::new(r"C:\Users\me\AppData\Local\Lanwork\cache"),
                version,
            )
            .is_none()
        );
        assert!(
            download_path(
                Path::new(r"C:\Users\me\AppData\Local\Lanwork\cache"),
                Path::new(r"C:\Users\me\Documents\Lanwork"),
                Path::new(r"C:\Users\me\AppData\Local\Lanwork\cache"),
                version,
            )
            .is_none()
        );
    }

    #[test]
    fn helper_script_quotes_paths_and_does_not_name_user_data() {
        let marker = Path::new(r"C:\Users\张三\AppData\Local\Lanwork\update-failed.txt");
        let script = update_helper_script(
            42,
            Path::new(r"C:\Temp\O'Brien\Lanwork-Setup-0.3.0.exe"),
            Path::new(r"C:\Users\张三\AppData\Local\Programs\Lanwork\lanwork.exe"),
            marker,
        )
        .unwrap();
        assert!(script.contains("Wait-Process -Id 42"));
        assert!(script.contains(r"'C:\Temp\O''Brien\Lanwork-Setup-0.3.0.exe'"));
        assert!(script.contains(r"'C:\Users\张三\AppData\Local\Programs\Lanwork\lanwork.exe'"));
        assert!(script.contains(r"'C:\Users\张三\AppData\Local\Lanwork\update-failed.txt'"));
        assert!(script.contains("$proc.ExitCode"));
        assert!(script.contains("Write-Error"));
        assert!(script.contains("Set-Content"));
        assert!(!script.contains("catch { }"));
        for arg in INSTALL_ARGS {
            assert!(script.contains(&format!("'{arg}'")), "{arg}");
        }
        assert!(!script.contains("/DIR"));
        assert!(!script.contains("Documents"));
        assert!(!script.contains("MayDolist"));
        assert!(!script.contains("LANWORK_DATA_DIR"));
        assert_eq!(
            update_helper_script(
                1,
                Path::new("a\nb.exe"),
                Path::new("c.exe"),
                Path::new("d.txt")
            ),
            None
        );
        assert_eq!(install_failure_log("2"), "安装失败：安装程序退出码 2");
        assert_eq!(install_failure_log(" 0 "), "安装失败：安装程序退出码 0");
        assert_eq!(install_failure_log("not-a-code"), "安装失败");
    }

    #[test]
    fn failure_marker_stays_out_of_the_data_directory() {
        let cache = PathBuf::from(r"C:\Users\me\AppData\Local\Lanwork").join("cache");
        let data = PathBuf::from(r"C:\Users\me\Documents\Lanwork");
        let temp = PathBuf::from(r"C:\Users\me\AppData\Local\Temp");
        let marker = install_failure_marker(&cache, &data, &temp).unwrap();
        assert_eq!(
            marker.file_name().and_then(|name| name.to_str()),
            Some("update-failed.txt")
        );
        assert_eq!(marker.parent(), cache.parent());
        let local = PathBuf::from(r"C:\Users\me\AppData\Local\Lanwork");
        assert!(install_failure_marker(&local.join("cache"), &local, &local).is_none());
    }

    fn temp_dir() -> PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "lanwork-update-{nanos}-{}",
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }
}
