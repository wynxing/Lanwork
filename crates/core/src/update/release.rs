//! 解析 `latest.json`，决定有没有可以安装的更新。

use serde::Deserialize;

use super::fetch::{
    NOTES_CHARS, installer_file_name, pinned_installer_url, release_page_allowed, url_allowed,
};
use super::install::sha256_eq;
use super::version::Version;

/// 核对过、可以下载的安装包。地址是钉死的第一跳，哈希来自 `latest.json`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedInstaller {
    pub name: String,
    pub url: String,
    pub sha256: String,
}

/// 比当前版本新的发布。`installer` 为空时不能静默安装，仍然可以打开发布页。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub version: Version,
    pub notes: String,
    pub release_url: String,
    pub installer: Option<VerifiedInstaller>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedManifest {
    /// `version` 不是稳定的三段版本。不当作更新。
    NotStable,
    File(LatestFile),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestFile {
    pub version: Version,
    pub notes: String,
    pub name: String,
    pub sha256: String,
}

pub fn parse_latest_json(bytes: &[u8]) -> Result<ParsedManifest, ()> {
    let file: LatestJson = serde_json::from_slice(bytes).map_err(|_| ())?;
    let Some(version) = Version::parse(&file.version) else {
        if file.version.trim().is_empty() {
            return Err(());
        }
        return Ok(ParsedManifest::NotStable);
    };
    Ok(ParsedManifest::File(LatestFile {
        version,
        notes: limit_notes(file.notes.as_deref().unwrap_or("")),
        name: file.installer.name,
        sha256: file.installer.sha256,
    }))
}

/// 版本不比当前新，就没有更新。安装包只有在文件名和哈希都对得上时才带上。
///
/// 下载地址按版本钉死，不读文件里的网址。
#[must_use]
pub fn manifest_update(current: Version, file: LatestFile) -> Option<AvailableUpdate> {
    if file.version <= current {
        return None;
    }
    let release_url = release_page(file.version);
    let release_url = if release_page_allowed(&release_url) {
        release_url
    } else {
        String::new()
    };
    let installer = installer_from(&file);
    Some(AvailableUpdate {
        version: file.version,
        notes: file.notes,
        release_url,
        installer,
    })
}

fn installer_from(file: &LatestFile) -> Option<VerifiedInstaller> {
    let name = installer_file_name(file.version);
    if file.name != name || !sha256_eq(&file.sha256, &file.sha256) {
        return None;
    }
    let url = pinned_installer_url(file.version);
    if !url_allowed(&url) {
        return None;
    }
    Some(VerifiedInstaller {
        name: file.name.clone(),
        url,
        sha256: file.sha256.to_ascii_lowercase(),
    })
}

fn release_page(version: Version) -> String {
    format!("https://github.com/wynxing/Lanwork/releases/tag/v{version}")
}

fn limit_notes(text: &str) -> String {
    let trimmed = text.trim();
    let mut out = String::new();
    for (index, ch) in trimmed.chars().enumerate() {
        if index >= NOTES_CHARS {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

#[derive(Deserialize)]
struct LatestJson {
    version: String,
    #[serde(default)]
    notes: Option<String>,
    installer: LatestInstallerJson,
}

#[derive(Deserialize)]
struct LatestInstallerJson {
    name: String,
    sha256: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::install::sha256_hex;

    const HASH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn newer_manifest_pins_the_installer_and_keeps_optional_notes() {
        let body = manifest("0.3.0", "Lanwork-Setup-0.3.0.exe", HASH, Some("  说明  "));
        let ParsedManifest::File(file) = parse_latest_json(body.as_bytes()).unwrap() else {
            panic!("stable");
        };
        let update = manifest_update(Version::new(0, 2, 0), file).unwrap();
        assert_eq!(update.version, Version::new(0, 3, 0));
        assert_eq!(update.notes, "说明");
        assert_eq!(
            update.release_url,
            "https://github.com/wynxing/Lanwork/releases/tag/v0.3.0"
        );
        let installer = update.installer.unwrap();
        assert_eq!(installer.sha256, HASH);
        assert_eq!(
            installer.url,
            "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/Lanwork-Setup-0.3.0.exe"
        );
        assert!(url_allowed(&installer.url));
    }

    #[test]
    fn a_manifest_without_notes_still_parses() {
        let body = format!(
            r#"{{"version":"0.3.0","installer":{{"name":"Lanwork-Setup-0.3.0.exe","sha256":"{HASH}"}}}}"#
        );
        let ParsedManifest::File(file) = parse_latest_json(body.as_bytes()).unwrap() else {
            panic!("stable");
        };
        assert!(file.notes.is_empty());
        assert!(
            manifest_update(Version::new(0, 2, 0), file)
                .unwrap()
                .installer
                .is_some()
        );
    }

    #[test]
    fn same_version_bad_hash_and_bad_name_do_not_install() {
        let same = file("0.2.0", "Lanwork-Setup-0.2.0.exe", HASH, None);
        assert!(manifest_update(Version::new(0, 2, 0), same).is_none());

        let wrong_hash = file("0.3.0", "Lanwork-Setup-0.3.0.exe", "zz", None);
        let update = manifest_update(Version::new(0, 2, 0), wrong_hash).unwrap();
        assert!(update.installer.is_none());

        let renamed = file("0.3.0", "other.exe", HASH, None);
        assert!(
            manifest_update(Version::new(0, 2, 0), renamed)
                .unwrap()
                .installer
                .is_none()
        );
    }

    #[test]
    fn prerelease_versions_are_not_an_update_and_bad_json_fails() {
        let body = format!(
            r#"{{"version":"0.9.0-rc1","installer":{{"name":"Lanwork-Setup-0.9.0.exe","sha256":"{HASH}"}}}}"#
        );
        assert_eq!(
            parse_latest_json(body.as_bytes()).unwrap(),
            ParsedManifest::NotStable
        );
        assert!(parse_latest_json(b"not json").is_err());
        assert!(
            parse_latest_json(br#"{"version":"","installer":{"name":"a","sha256":"zz"}}"#).is_err()
        );
    }

    #[test]
    fn notes_are_trimmed_and_capped() {
        let long = "字".repeat(NOTES_CHARS + 10);
        let file = file("0.3.0", "Lanwork-Setup-0.3.0.exe", HASH, Some(&long));
        let update = manifest_update(Version::new(0, 2, 0), file).unwrap();
        assert_eq!(update.notes.chars().count(), NOTES_CHARS + 1);
        assert!(update.notes.ends_with('…'));
    }

    #[test]
    fn uppercase_hash_is_normalized() {
        let file = file(
            "0.3.0",
            "Lanwork-Setup-0.3.0.exe",
            &sha256_hex(b"setup").to_ascii_uppercase(),
            None,
        );
        let update = manifest_update(Version::new(0, 2, 0), file).unwrap();
        assert_eq!(update.installer.unwrap().sha256, sha256_hex(b"setup"));
    }

    fn file(version: &str, name: &str, hash: &str, notes: Option<&str>) -> LatestFile {
        let body = manifest(version, name, hash, notes);
        match parse_latest_json(body.as_bytes()).unwrap() {
            ParsedManifest::File(file) => file,
            ParsedManifest::NotStable => panic!("stable version"),
        }
    }

    fn manifest(version: &str, name: &str, hash: &str, notes: Option<&str>) -> String {
        let notes = notes
            .map(|text| format!(r#","notes":{}"#, serde_json::to_string(text).unwrap()))
            .unwrap_or_default();
        format!(
            r#"{{"version":"{version}"{notes},"installer":{{"name":"{name}","sha256":"{hash}"}}}}"#
        )
    }
}
