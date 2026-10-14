//! 应用内更新的决定。
//!
//! 读取 `wynxing/Lanwork` 最新正式版上的 `latest.json`，核对安装包 SHA-256。
//! 便携版只打开发布页。下载和启动安装程序在 `crates/app`。
//! 这里不写数据目录，也不宣称更新已经验收。

mod fetch;
mod install;
mod release;
mod version;
mod view;

pub use fetch::{
    CHECK_INTERVAL, GetBytes, HttpsUrl, LATEST_MANIFEST_URL, MAX_INSTALLER_BYTES,
    MAX_METADATA_BYTES, MAX_REDIRECTS, NOTES_CHARS, STARTUP_DELAY, TransportError,
    installer_file_name, parse_https_url, pinned_installer_url, pinned_latest_json_url,
    redirect_allowed, release_page_allowed, request_headers, resolve_redirect, url_allowed,
};
pub use install::{
    ContentHash, Edition, INSTALL_ARGS, download_path, edition_of, install_failure_log,
    install_failure_marker, review_download, sha256_eq, sha256_hex, update_helper_script,
};
pub use release::{AvailableUpdate, VerifiedInstaller};
pub use version::{Version, current_version};
pub use view::{
    ButtonAction, CheckOutcome, Phase, TEXT_BUTTON_CHECK, TEXT_BUTTON_INSTALL, TEXT_BUTTON_OPEN,
    TEXT_CHECK_FAILED, TEXT_CHECKING, TEXT_CHECKSUM_UNAVAILABLE, TEXT_DOWNLOAD_FAILED,
    TEXT_INSTALL_FAILED, TEXT_INSTALLING, TEXT_OPEN_FAILED, TEXT_UP_TO_DATE, TEXT_VERIFY_FAILED,
    TEXT_VERIFYING, UpdateSession, UpdateUi, download_status, present,
};

use release::{ParsedManifest, manifest_update, parse_latest_json};

/// 读一次最新正式版的 `latest.json`。网络和解析失败都变成「检查更新失败」，不产生安装计划。
pub fn run_check(transport: &impl GetBytes, current: Version) -> CheckOutcome {
    let body = match transport.get(LATEST_MANIFEST_URL, MAX_METADATA_BYTES) {
        Ok(body) => body,
        Err(_) => return CheckOutcome::Failed(TEXT_CHECK_FAILED),
    };
    let parsed = match parse_latest_json(&body) {
        Ok(parsed) => parsed,
        Err(()) => return CheckOutcome::Failed(TEXT_CHECK_FAILED),
    };
    let file = match parsed {
        ParsedManifest::NotStable => return CheckOutcome::Current,
        ParsedManifest::File(file) => file,
    };
    match manifest_update(current, file) {
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
    fn a_newer_manifest_is_installed_only_when_the_checksum_matches() {
        let hash = sha256_hex(b"setup-bytes");
        let latest = manifest("0.3.0", "Lanwork-Setup-0.3.0.exe", &hash, Some("说明"));
        let fake = fake(&latest);
        let outcome = run_check(&fake, Version::new(0, 2, 0));
        let CheckOutcome::Available(update) = outcome else {
            panic!("expected an update");
        };
        let installer = update.installer.expect("installer");
        assert_eq!(installer.sha256, hash);
        assert_eq!(update.notes, "说明");
        assert_eq!(installer.url, pinned_installer_url(Version::new(0, 3, 0)));
        assert!(review_download(&installer.sha256, &hash, b"setup-bytes".len() as u64).is_ok());
        assert_eq!(
            review_download(&installer.sha256, &sha256_hex(b"tampered"), 8),
            Err(TEXT_VERIFY_FAILED)
        );
        assert_eq!(fake.hits.borrow().as_slice(), [LATEST_MANIFEST_URL]);
    }

    #[test]
    fn the_published_manifest_shape_without_notes_still_offers_an_install() {
        let hash = "ab".repeat(32);
        let latest = manifest("0.3.0", "Lanwork-Setup-0.3.0.exe", &hash, None);
        assert!(!latest.contains("notes"));
        let fake = fake(&latest);
        let CheckOutcome::Available(update) = run_check(&fake, Version::new(0, 2, 0)) else {
            panic!("expected an update");
        };
        assert!(update.notes.is_empty());
        assert!(update.installer.is_some());
        assert_eq!(fake.hits.borrow().len(), 1);
    }

    #[test]
    fn the_current_version_is_not_an_update() {
        let latest = manifest("0.2.0", "Lanwork-Setup-0.2.0.exe", &"ab".repeat(32), None);
        let fake = fake(&latest);
        assert_eq!(
            run_check(&fake, Version::new(0, 2, 0)),
            CheckOutcome::Current
        );
        assert_eq!(fake.hits.borrow().as_slice(), [LATEST_MANIFEST_URL]);
    }

    #[test]
    fn network_failure_and_a_bad_checksum_do_not_start_an_install() {
        let offline = Fake {
            files: HashMap::new(),
            hits: RefCell::new(Vec::new()),
        };
        assert_eq!(
            run_check(&offline, Version::new(0, 2, 0)),
            CheckOutcome::Failed(TEXT_CHECK_FAILED)
        );

        let latest = manifest("0.3.0", "Lanwork-Setup-0.3.0.exe", "zz", Some("说明"));
        let bad_hash = fake(&latest);
        let CheckOutcome::Available(update) = run_check(&bad_hash, Version::new(0, 2, 0)) else {
            panic!("version is still shown");
        };
        assert!(update.installer.is_none());
        assert_eq!(update.version, Version::new(0, 3, 0));
        assert_eq!(update.notes, "说明");
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
    fn a_prerelease_manifest_is_not_offered() {
        let latest = manifest(
            "0.9.0-rc1",
            "Lanwork-Setup-0.9.0-rc1.exe",
            &"ab".repeat(32),
            None,
        );
        let fake = fake(&latest);
        assert_eq!(
            run_check(&fake, Version::new(0, 2, 0)),
            CheckOutcome::Current
        );
        assert_eq!(fake.hits.borrow().as_slice(), [LATEST_MANIFEST_URL]);
    }

    fn fake(latest: &str) -> Fake {
        Fake {
            files: HashMap::from([(
                LATEST_MANIFEST_URL.to_owned(),
                Ok(latest.as_bytes().to_vec()),
            )]),
            hits: RefCell::new(Vec::new()),
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
