//! 解析 GitHub Release 和 `latest.json`，决定有没有可以安装的更新。

use serde::Deserialize;

use super::fetch::{
    NOTES_CHARS, installer_file_name, pinned_installer_url, pinned_latest_json_url,
    release_page_allowed, url_allowed,
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

/// 比当前版本新的 Release。`installer` 为空时不能静默安装，仍然可以打开发布页。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub version: Version,
    pub notes: String,
    pub release_url: String,
    pub installer: Option<VerifiedInstaller>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParsedRelease {
    /// 标签不是稳定的三段版本。不当作更新。
    NotStable,
    Stable(GithubRelease),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GithubRelease {
    pub version: Version,
    pub notes: String,
    pub release_url: String,
    pub latest_json_url: Option<String>,
    pub installer_url: Option<String>,
    /// 资产上的 `digest`，已经收成小写十六进制。没有这项就是 `None`。
    pub digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatestFile {
    pub version: Version,
    pub name: String,
    pub sha256: String,
}

pub fn parse_github_release(bytes: &[u8]) -> Result<ParsedRelease, ()> {
    let release: ReleaseJson = serde_json::from_slice(bytes).map_err(|_| ())?;
    let Some(version) = Version::parse(&release.tag_name) else {
        return Ok(ParsedRelease::NotStable);
    };
    let name = installer_file_name(version);
    let mut latest_json_url = None;
    let mut installer_url = None;
    let mut digest = None;
    for asset in release.assets.unwrap_or_default() {
        if asset.name == "latest.json" && latest_json_url.is_none() {
            latest_json_url = exact_pinned_url(version, "latest.json", &asset.browser_download_url);
        } else if asset.name == name && installer_url.is_none() {
            installer_url = exact_pinned_url(version, &name, &asset.browser_download_url);
            digest = asset.digest.as_deref().and_then(normalize_digest);
        }
    }
    let release_url = release
        .html_url
        .as_deref()
        .filter(|url| release_page_allowed(url))
        .unwrap_or("")
        .to_owned();
    Ok(ParsedRelease::Stable(GithubRelease {
        version,
        notes: limit_notes(release.body.as_deref().unwrap_or("")),
        release_url,
        latest_json_url,
        installer_url,
        digest,
    }))
}

pub fn parse_latest_json(bytes: &[u8]) -> Result<LatestFile, ()> {
    let file: LatestJson = serde_json::from_slice(bytes).map_err(|_| ())?;
    let version = Version::parse(&file.version).ok_or(())?;
    if !sha256_eq(&file.installer.sha256, &file.installer.sha256) {
        return Err(());
    }
    Ok(LatestFile {
        version,
        name: file.installer.name,
        sha256: file.installer.sha256.to_ascii_lowercase(),
    })
}

/// 版本不比当前新，就没有更新。安装包只有在文件名、哈希和地址都对得上时才带上。
#[must_use]
pub fn assemble(
    current: Version,
    github: GithubRelease,
    latest: Option<LatestFile>,
) -> Option<AvailableUpdate> {
    if github.version <= current {
        return None;
    }
    let installer = installer_from(&github, latest);
    Some(AvailableUpdate {
        version: github.version,
        notes: github.notes,
        release_url: github.release_url,
        installer,
    })
}

fn installer_from(github: &GithubRelease, latest: Option<LatestFile>) -> Option<VerifiedInstaller> {
    let latest = latest?;
    let expected_name = installer_file_name(github.version);
    if latest.version != github.version || latest.name != expected_name {
        return None;
    }
    if let Some(digest) = &github.digest
        && !sha256_eq(digest, &latest.sha256)
    {
        return None;
    }
    let url = github.installer_url.clone()?;
    if !url_allowed(&url) {
        return None;
    }
    Some(VerifiedInstaller {
        name: latest.name,
        url,
        sha256: latest.sha256,
    })
}

/// Release 里的地址只有和钉死的第一跳完全一致时才采用。别的路径、CDN 和 `..` 都丢掉。
fn exact_pinned_url(version: Version, name: &str, browser_url: &str) -> Option<String> {
    let pinned = if name == "latest.json" {
        pinned_latest_json_url(version)
    } else if name == installer_file_name(version) {
        pinned_installer_url(version)
    } else {
        return None;
    };
    (browser_url == pinned && url_allowed(&pinned)).then_some(pinned)
}

fn normalize_digest(digest: &str) -> Option<String> {
    let hex = digest
        .trim()
        .strip_prefix("sha256:")
        .or_else(|| digest.trim().strip_prefix("SHA256:"))?;
    sha256_eq(hex, hex).then(|| hex.to_ascii_lowercase())
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
struct ReleaseJson {
    tag_name: String,
    body: Option<String>,
    html_url: Option<String>,
    assets: Option<Vec<AssetJson>>,
}

#[derive(Deserialize)]
struct AssetJson {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

#[derive(Deserialize)]
struct LatestJson {
    version: String,
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
    fn newer_release_needs_a_matching_latest_json() {
        let github = stable("v0.3.0", Some(HASH));
        let latest = LatestFile {
            version: Version::new(0, 3, 0),
            name: "Lanwork-Setup-0.3.0.exe".to_owned(),
            sha256: HASH.to_owned(),
        };
        let update = assemble(Version::new(0, 2, 0), github, Some(latest)).unwrap();
        assert_eq!(update.version, Version::new(0, 3, 0));
        assert_eq!(update.notes, "说明");
        let installer = update.installer.unwrap();
        assert_eq!(installer.sha256, HASH);
        assert!(installer.url.contains("Lanwork-Setup-0.3.0.exe"));
    }

    #[test]
    fn same_version_digest_mismatch_and_bad_name_do_not_install() {
        let github = stable("v0.2.0", Some(HASH));
        assert!(assemble(Version::new(0, 2, 0), github, None).is_none());

        let github = stable("v0.3.0", Some(HASH));
        let wrong = LatestFile {
            version: Version::new(0, 3, 0),
            name: "Lanwork-Setup-0.3.0.exe".to_owned(),
            sha256: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        };
        let update = assemble(Version::new(0, 2, 0), github, Some(wrong)).unwrap();
        assert!(update.installer.is_none());

        let github = stable("v0.3.0", None);
        let renamed = LatestFile {
            version: Version::new(0, 3, 0),
            name: "other.exe".to_owned(),
            sha256: HASH.to_owned(),
        };
        assert!(
            assemble(Version::new(0, 2, 0), github, Some(renamed))
                .unwrap()
                .installer
                .is_none()
        );
    }

    #[test]
    fn prerelease_tags_are_not_an_update() {
        let body = br#"{"tag_name":"v0.3.0-rc1","html_url":"https://github.com/wynxing/Lanwork/releases/tag/v0.3.0-rc1","assets":[]}"#;
        assert_eq!(
            parse_github_release(body).unwrap(),
            ParsedRelease::NotStable
        );
        assert!(parse_github_release(b"not json").is_err());
    }

    #[test]
    fn notes_are_trimmed_and_capped() {
        let long = "字".repeat(NOTES_CHARS + 10);
        let github = GithubRelease {
            version: Version::new(0, 3, 0),
            notes: limit_notes(&format!("  {long}  ")),
            release_url: String::new(),
            latest_json_url: None,
            installer_url: None,
            digest: None,
        };
        let update = assemble(Version::new(0, 2, 0), github, None).unwrap();
        assert_eq!(update.notes.chars().count(), NOTES_CHARS + 1);
        assert!(update.notes.ends_with('…'));
    }

    #[test]
    fn substituted_download_urls_are_not_the_pinned_first_hop() {
        let github = stable("v0.3.0", Some(HASH));
        assert_eq!(
            github.installer_url.as_deref(),
            Some(
                "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/Lanwork-Setup-0.3.0.exe"
            )
        );
        assert_eq!(
            github.latest_json_url.as_deref(),
            Some("https://github.com/wynxing/Lanwork/releases/download/v0.3.0/latest.json")
        );

        let cdn = release_with_urls(
            "v0.3.0",
            "https://release-assets.githubusercontent.com/github-production-release-asset/abcd",
            "https://objects.githubusercontent.com/latest.json",
        );
        assert!(cdn.installer_url.is_none());
        assert!(cdn.latest_json_url.is_none());

        let dotdot = release_with_urls(
            "v0.3.0",
            "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/../Lanwork-Setup-0.3.0.exe",
            "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/%2e%2e/latest.json",
        );
        assert!(dotdot.installer_url.is_none());
        assert!(dotdot.latest_json_url.is_none());

        let other = release_with_urls(
            "v0.3.0",
            "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/other.exe",
            "https://github.com/wynxing/Lanwork/releases/download/v0.2.0/latest.json",
        );
        assert!(other.installer_url.is_none());
        assert!(other.latest_json_url.is_none());
        let update = assemble(Version::new(0, 2, 0), other, None).unwrap();
        assert!(update.installer.is_none());
    }

    #[test]
    fn latest_json_rejects_a_bad_hash() {
        let json = format!(
            r#"{{"version":"0.3.0","installer":{{"name":"Lanwork-Setup-0.3.0.exe","sha256":"{}"}}}}"#,
            sha256_hex(b"setup")
        );
        let file = parse_latest_json(json.as_bytes()).unwrap();
        assert_eq!(file.sha256, sha256_hex(b"setup"));
        assert!(
            parse_latest_json(br#"{"version":"0.3.0","installer":{"name":"a","sha256":"zz"}}"#)
                .is_err()
        );
    }

    fn release_with_urls(tag: &str, installer_url: &str, latest_url: &str) -> GithubRelease {
        let body = format!(
            r#"{{
                "tag_name":"{tag}",
                "body":"说明",
                "html_url":"https://github.com/wynxing/Lanwork/releases/tag/{tag}",
                "assets":[
                    {{
                        "name":"Lanwork-Setup-0.3.0.exe",
                        "browser_download_url":"{installer_url}"
                    }},
                    {{
                        "name":"latest.json",
                        "browser_download_url":"{latest_url}"
                    }}
                ]
            }}"#
        );
        match parse_github_release(body.as_bytes()).unwrap() {
            ParsedRelease::Stable(release) => release,
            ParsedRelease::NotStable => panic!("stable tag"),
        }
    }

    fn stable(tag: &str, digest: Option<&str>) -> GithubRelease {
        let digest_json = digest
            .map(|hash| format!(r#","digest":"sha256:{hash}""#))
            .unwrap_or_default();
        let body = format!(
            r#"{{
                "tag_name":"{tag}",
                "body":"说明",
                "html_url":"https://github.com/wynxing/Lanwork/releases/tag/{tag}",
                "assets":[
                    {{
                        "name":"Lanwork-Setup-0.3.0.exe",
                        "browser_download_url":"https://github.com/wynxing/Lanwork/releases/download/v0.3.0/Lanwork-Setup-0.3.0.exe"{digest_json}
                    }},
                    {{
                        "name":"latest.json",
                        "browser_download_url":"https://github.com/wynxing/Lanwork/releases/download/v0.3.0/latest.json"
                    }},
                    {{
                        "name":"Lanwork-0.3.0-portable-x64.zip",
                        "browser_download_url":"https://evil.example/setup.exe"
                    }}
                ]
            }}"#
        );
        match parse_github_release(body.as_bytes()).unwrap() {
            ParsedRelease::Stable(release) => release,
            ParsedRelease::NotStable => panic!("stable tag"),
        }
    }
}
