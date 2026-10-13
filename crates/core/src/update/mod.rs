//! 应用内更新的决定。
//!
//! 检查 `wynxing/Lanwork` 的最新 GitHub Release，用同一 Release 上的 `latest.json`
//! 核对安装包 SHA-256。便携版只打开发布页。下载和启动安装程序在 `crates/app`。
//! 这里不写数据目录，也不宣称更新已经验收。

mod fetch;
mod install;
mod release;
mod version;
mod view;

pub use fetch::{
    CHECK_INTERVAL, GetBytes, HttpsUrl, LATEST_RELEASE_URL, MAX_INSTALLER_BYTES,
    MAX_METADATA_BYTES, MAX_REDIRECTS, NOTES_CHARS, STARTUP_DELAY, TransportError,
    installer_file_name, parse_https_url, release_page_allowed, resolve_redirect, url_allowed,
};
pub use install::{
    ContentHash, Edition, INSTALL_ARGS, download_path, edition_of, review_download, sha256_eq,
    sha256_hex, update_helper_script,
};
pub use release::{AvailableUpdate, VerifiedInstaller};
pub use version::{Version, current_version};
pub use view::{
    ButtonAction, CheckOutcome, Phase, TEXT_BUTTON_CHECK, TEXT_BUTTON_INSTALL, TEXT_BUTTON_OPEN,
    TEXT_CHECK_FAILED, TEXT_CHECKING, TEXT_CHECKSUM_UNAVAILABLE, TEXT_DOWNLOAD_FAILED,
    TEXT_INSTALL_FAILED, TEXT_INSTALLING, TEXT_OPEN_FAILED, TEXT_UP_TO_DATE, TEXT_VERIFY_FAILED,
    TEXT_VERIFYING, UpdateSession, UpdateUi, download_status, present,
};

use release::{ParsedRelease, assemble, parse_github_release, parse_latest_json};

/// 向 GitHub 查一次更新。网络和解析失败都变成「检查更新失败」，不产生安装计划。
pub fn run_check(transport: &impl GetBytes, current: Version) -> CheckOutcome {
    let body = match transport.get(LATEST_RELEASE_URL, MAX_METADATA_BYTES) {
        Ok(body) => body,
        Err(_) => return CheckOutcome::Failed(TEXT_CHECK_FAILED),
    };
    let parsed = match parse_github_release(&body) {
        Ok(parsed) => parsed,
        Err(()) => return CheckOutcome::Failed(TEXT_CHECK_FAILED),
    };
    let github = match parsed {
        ParsedRelease::NotStable => return CheckOutcome::Current,
        ParsedRelease::Stable(github) => github,
    };
    if github.version <= current {
        return CheckOutcome::Current;
    }
    let latest_url = github.latest_json_url.clone();
    let latest = latest_url.and_then(|url| {
        if !url_allowed(&url) {
            return None;
        }
        let bytes = transport.get(&url, MAX_METADATA_BYTES).ok()?;
        parse_latest_json(&bytes).ok()
    });
    match assemble(current, github, latest) {
        Some(update) => CheckOutcome::Available(update),
        None => CheckOutcome::Current,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use super::*;

    struct Fake {
        files: HashMap<String, Result<Vec<u8>, TransportError>>,
        hits: RefCell<Vec<String>>,
    }

    impl GetBytes for Fake {
        fn get(&self, url: &str, limit: u64) -> Result<Vec<u8>, TransportError> {
            assert!(limit > 0);
            self.hits.borrow_mut().push(url.to_owned());
            self.files
                .get(url)
                .cloned()
                .unwrap_or(Err(TransportError::Network))
        }
    }

    #[test]
    fn a_newer_release_is_installed_only_when_the_checksum_matches() {
        let hash = sha256_hex(b"setup-bytes");
        let api = release_json("v0.3.0", &hash);
        let latest = format!(
            r#"{{"version":"0.3.0","installer":{{"name":"Lanwork-Setup-0.3.0.exe","sha256":"{hash}"}}}}"#
        );
        let fake = fake(&api, Some(&latest));
        let outcome = run_check(&fake, Version::new(0, 2, 0));
        let CheckOutcome::Available(update) = outcome else {
            panic!("expected an update");
        };
        let installer = update.installer.expect("installer");
        assert_eq!(installer.sha256, hash);
        assert!(review_download(&installer.sha256, &hash, b"setup-bytes".len() as u64).is_ok());
        assert_eq!(
            review_download(&installer.sha256, &sha256_hex(b"tampered"), 8),
            Err(TEXT_VERIFY_FAILED)
        );
        assert_eq!(fake.hits.borrow().len(), 2);
    }

    #[test]
    fn the_current_version_does_not_download_latest_json() {
        let api = release_json("v0.2.0", &"ab".repeat(32));
        let fake = fake(&api, None);
        assert_eq!(
            run_check(&fake, Version::new(0, 2, 0)),
            CheckOutcome::Current
        );
        assert_eq!(fake.hits.borrow().len(), 1);
    }

    #[test]
    fn network_failure_and_a_missing_checksum_do_not_start_an_install() {
        let fake = Fake {
            files: HashMap::new(),
            hits: RefCell::new(Vec::new()),
        };
        assert_eq!(
            run_check(&fake, Version::new(0, 2, 0)),
            CheckOutcome::Failed(TEXT_CHECK_FAILED)
        );

        let api = release_json("v0.3.0", &"ab".repeat(32));
        let mut files = HashMap::new();
        files.insert(LATEST_RELEASE_URL.to_owned(), Ok(api.into_bytes()));
        files.insert(
            "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/latest.json".to_owned(),
            Err(TransportError::Network),
        );
        let fake = Fake {
            files,
            hits: RefCell::new(Vec::new()),
        };
        let CheckOutcome::Available(update) = run_check(&fake, Version::new(0, 2, 0)) else {
            panic!("version is still shown");
        };
        assert!(update.installer.is_none());
        assert_eq!(update.version, Version::new(0, 3, 0));
        let portable = present(
            Version::new(0, 2, 0),
            Edition::Portable,
            &Phase::Available(update.clone()),
        );
        assert_eq!(portable.action, ButtonAction::OpenRelease);
        let installed = present(
            Version::new(0, 2, 0),
            Edition::Installed,
            &Phase::Available(update),
        );
        assert_ne!(installed.action, ButtonAction::Install);
    }

    #[test]
    fn a_prerelease_latest_is_not_offered() {
        let api = r#"{"tag_name":"v0.9.0-rc1","body":"rc","html_url":"https://github.com/wynxing/Lanwork/releases/tag/v0.9.0-rc1","assets":[]}"#;
        let fake = Fake {
            files: HashMap::from([(LATEST_RELEASE_URL.to_owned(), Ok(api.as_bytes().to_vec()))]),
            hits: RefCell::new(Vec::new()),
        };
        assert_eq!(
            run_check(&fake, Version::new(0, 2, 0)),
            CheckOutcome::Current
        );
    }

    fn fake(api: &str, latest: Option<&str>) -> Fake {
        let mut files = HashMap::new();
        files.insert(LATEST_RELEASE_URL.to_owned(), Ok(api.as_bytes().to_vec()));
        if let Some(latest) = latest {
            files.insert(
                "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/latest.json"
                    .to_owned(),
                Ok(latest.as_bytes().to_vec()),
            );
        }
        Fake {
            files,
            hits: RefCell::new(Vec::new()),
        }
    }

    fn release_json(tag: &str, hash: &str) -> String {
        format!(
            r#"{{
                "tag_name":"{tag}",
                "body":"说明",
                "html_url":"https://github.com/wynxing/Lanwork/releases/tag/{tag}",
                "assets":[
                    {{
                        "name":"Lanwork-Setup-0.3.0.exe",
                        "browser_download_url":"https://github.com/wynxing/Lanwork/releases/download/v0.3.0/Lanwork-Setup-0.3.0.exe",
                        "digest":"sha256:{hash}"
                    }},
                    {{
                        "name":"latest.json",
                        "browser_download_url":"https://github.com/wynxing/Lanwork/releases/download/v0.3.0/latest.json"
                    }}
                ]
            }}"#
        )
    }
}
