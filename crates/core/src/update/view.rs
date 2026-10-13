//! 设置页和托盘要显示的更新状态。不访问网络。

use super::install::Edition;
use super::release::AvailableUpdate;
use super::version::Version;

pub const TEXT_CHECK_FAILED: &str = "检查更新失败";
pub const TEXT_DOWNLOAD_FAILED: &str = "下载失败";
pub const TEXT_VERIFY_FAILED: &str = "校验失败";
pub const TEXT_INSTALL_FAILED: &str = "安装失败";
pub const TEXT_OPEN_FAILED: &str = "打开发布页失败";
pub const TEXT_CHECKSUM_UNAVAILABLE: &str = "校验信息不可用";
pub const TEXT_UP_TO_DATE: &str = "已是最新版本";
pub const TEXT_CHECKING: &str = "正在检查";
pub const TEXT_VERIFYING: &str = "正在校验";
pub const TEXT_INSTALLING: &str = "正在安装";
pub const TEXT_BUTTON_CHECK: &str = "检查更新";
pub const TEXT_BUTTON_INSTALL: &str = "更新";
pub const TEXT_BUTTON_OPEN: &str = "打开发布页";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    Current,
    Available(AvailableUpdate),
    Failed(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Checking {
        tray: bool,
    },
    Current,
    Available(AvailableUpdate),
    Downloading {
        update: AvailableUpdate,
        received: u64,
        total: Option<u64>,
    },
    Verifying(AvailableUpdate),
    Installing(AvailableUpdate),
    Failed {
        message: &'static str,
        update: Option<AvailableUpdate>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonAction {
    Check,
    Install,
    OpenRelease,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateUi {
    pub version_text: String,
    pub status: String,
    pub status_error: bool,
    pub notes: String,
    pub notes_visible: bool,
    pub button: &'static str,
    pub button_enabled: bool,
    pub tray: bool,
    pub action: ButtonAction,
}

/// 界面状态。自动检查失败时，如果已经知道有更新，就留着那一次的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateSession {
    phase: Phase,
    remembered: Option<AvailableUpdate>,
}

impl UpdateSession {
    #[must_use]
    pub fn new() -> Self {
        Self {
            phase: Phase::Idle,
            remembered: None,
        }
    }

    #[must_use]
    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn begin_check(&mut self) {
        let tray = shows_tray(&self.phase);
        self.phase = Phase::Checking { tray };
    }

    pub fn finish_check(&mut self, outcome: CheckOutcome, manual: bool) {
        match outcome {
            CheckOutcome::Current => {
                self.remembered = None;
                self.phase = Phase::Current;
            }
            CheckOutcome::Available(update) => {
                self.remembered = Some(update.clone());
                self.phase = Phase::Available(update);
            }
            CheckOutcome::Failed(message) => {
                if !manual && let Some(update) = self.remembered.clone() {
                    self.phase = Phase::Available(update);
                    return;
                }
                self.phase = Phase::Failed {
                    message,
                    update: self.remembered.clone(),
                };
            }
        }
    }

    pub fn begin_download(&mut self, update: AvailableUpdate) {
        self.phase = Phase::Downloading {
            update,
            received: 0,
            total: None,
        };
    }

    pub fn download_progress(&mut self, received: u64, total: Option<u64>) {
        let Phase::Downloading { update, .. } = &self.phase else {
            return;
        };
        let update = update.clone();
        self.phase = Phase::Downloading {
            update,
            received,
            total,
        };
    }

    pub fn begin_verify(&mut self) {
        let Some(update) = self.current_update().cloned() else {
            return;
        };
        self.phase = Phase::Verifying(update);
    }

    pub fn begin_install_phase(&mut self) {
        let Some(update) = self.current_update().cloned() else {
            return;
        };
        self.phase = Phase::Installing(update);
    }

    pub fn fail(&mut self, message: &'static str) {
        self.phase = Phase::Failed {
            message,
            update: self.remembered.clone(),
        };
    }

    #[must_use]
    pub fn current_update(&self) -> Option<&AvailableUpdate> {
        match &self.phase {
            Phase::Available(update)
            | Phase::Downloading { update, .. }
            | Phase::Verifying(update)
            | Phase::Installing(update)
            | Phase::Failed {
                update: Some(update),
                ..
            } => Some(update),
            Phase::Idle | Phase::Checking { .. } | Phase::Current | Phase::Failed { .. } => {
                self.remembered.as_ref()
            }
        }
    }
}

impl Default for UpdateSession {
    fn default() -> Self {
        Self::new()
    }
}

#[must_use]
pub fn present(current: Version, edition: Edition, phase: &Phase) -> UpdateUi {
    let controls = controls(edition, phase);
    UpdateUi {
        version_text: current.to_string(),
        status: controls.status,
        status_error: controls.status_error,
        notes: controls.notes,
        notes_visible: controls.notes_visible,
        button: controls.button,
        button_enabled: controls.enabled,
        tray: controls.tray,
        action: controls.action,
    }
}

#[must_use]
pub fn download_status(received: u64, total: Option<u64>) -> String {
    match total {
        Some(total) if total > 0 => {
            let pct = (received.saturating_mul(100) / total).min(100);
            format!("正在下载 {pct}%")
        }
        _ => "正在下载".to_owned(),
    }
}

struct Controls {
    status: String,
    status_error: bool,
    notes: String,
    notes_visible: bool,
    button: &'static str,
    enabled: bool,
    tray: bool,
    action: ButtonAction,
}

fn controls(edition: Edition, phase: &Phase) -> Controls {
    match phase {
        Phase::Idle => plain(String::new(), false, TEXT_BUTTON_CHECK, true, false),
        Phase::Checking { tray } => {
            let mut row = plain(
                TEXT_CHECKING.to_owned(),
                false,
                TEXT_BUTTON_CHECK,
                false,
                *tray,
            );
            row.action = ButtonAction::None;
            row
        }
        Phase::Current => plain(
            TEXT_UP_TO_DATE.to_owned(),
            false,
            TEXT_BUTTON_CHECK,
            true,
            false,
        ),
        Phase::Available(update) => available_controls(edition, update, None),
        Phase::Downloading {
            update,
            received,
            total,
        } => busy(
            update,
            download_status(*received, *total),
            false,
            TEXT_BUTTON_INSTALL,
        ),
        Phase::Verifying(update) => busy(
            update,
            TEXT_VERIFYING.to_owned(),
            false,
            TEXT_BUTTON_INSTALL,
        ),
        Phase::Installing(update) => busy(
            update,
            TEXT_INSTALLING.to_owned(),
            false,
            TEXT_BUTTON_INSTALL,
        ),
        Phase::Failed { message, update } => match update {
            Some(update) => available_controls(edition, update, Some(*message)),
            None => plain((*message).to_owned(), true, TEXT_BUTTON_CHECK, true, false),
        },
    }
}

fn available_controls(
    edition: Edition,
    update: &AvailableUpdate,
    error: Option<&'static str>,
) -> Controls {
    let (button, action) = action_for(edition, update);
    let (status, status_error) = if let Some(message) = error {
        (message.to_owned(), true)
    } else if edition == Edition::Installed && update.installer.is_none() {
        (
            format!("{}　{TEXT_CHECKSUM_UNAVAILABLE}", update.version),
            true,
        )
    } else {
        (update.version.to_string(), false)
    };
    Controls {
        status,
        status_error,
        notes: update.notes.clone(),
        notes_visible: !update.notes.is_empty(),
        button,
        enabled: true,
        tray: true,
        action,
    }
}

fn action_for(edition: Edition, update: &AvailableUpdate) -> (&'static str, ButtonAction) {
    if edition == Edition::Installed && update.installer.is_some() {
        (TEXT_BUTTON_INSTALL, ButtonAction::Install)
    } else if super::fetch::release_page_allowed(&update.release_url) {
        (TEXT_BUTTON_OPEN, ButtonAction::OpenRelease)
    } else {
        (TEXT_BUTTON_CHECK, ButtonAction::Check)
    }
}

fn busy(
    update: &AvailableUpdate,
    status: String,
    status_error: bool,
    button: &'static str,
) -> Controls {
    Controls {
        status,
        status_error,
        notes: update.notes.clone(),
        notes_visible: !update.notes.is_empty(),
        button,
        enabled: false,
        tray: true,
        action: ButtonAction::None,
    }
}

fn plain(
    status: String,
    status_error: bool,
    button: &'static str,
    enabled: bool,
    tray: bool,
) -> Controls {
    Controls {
        status,
        status_error,
        notes: String::new(),
        notes_visible: false,
        button,
        enabled,
        tray,
        action: if enabled {
            ButtonAction::Check
        } else {
            ButtonAction::None
        },
    }
}

fn shows_tray(phase: &Phase) -> bool {
    match phase {
        Phase::Checking { tray } => *tray,
        Phase::Available(_)
        | Phase::Downloading { .. }
        | Phase::Verifying(_)
        | Phase::Installing(_) => true,
        Phase::Failed { update, .. } => update.is_some(),
        Phase::Idle | Phase::Current => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::release::VerifiedInstaller;

    fn sample() -> AvailableUpdate {
        AvailableUpdate {
            version: Version::new(0, 3, 0),
            notes: "修复".to_owned(),
            release_url: "https://github.com/wynxing/Lanwork/releases/tag/v0.3.0".to_owned(),
            installer: Some(VerifiedInstaller {
                name: "Lanwork-Setup-0.3.0.exe".to_owned(),
                url: "https://github.com/wynxing/Lanwork/releases/download/v0.3.0/Lanwork-Setup-0.3.0.exe".to_owned(),
                sha256: "ab".repeat(32),
            }),
        }
    }

    #[test]
    fn installed_edition_waits_for_the_update_button() {
        let ui = present(
            Version::new(0, 2, 0),
            Edition::Installed,
            &Phase::Available(sample()),
        );
        assert_eq!(ui.version_text, "0.2.0");
        assert_eq!(ui.status, "0.3.0");
        assert_eq!(ui.notes, "修复");
        assert!(ui.notes_visible);
        assert_eq!(ui.button, TEXT_BUTTON_INSTALL);
        assert!(ui.button_enabled);
        assert!(ui.tray);
        assert_eq!(ui.action, ButtonAction::Install);
        assert!(!ui.status_error);
    }

    #[test]
    fn portable_edition_only_opens_the_release_page() {
        let ui = present(
            Version::new(0, 2, 0),
            Edition::Portable,
            &Phase::Available(sample()),
        );
        assert_eq!(ui.button, TEXT_BUTTON_OPEN);
        assert_eq!(ui.action, ButtonAction::OpenRelease);
        assert!(ui.tray);
    }

    #[test]
    fn missing_checksum_does_not_offer_install() {
        let mut update = sample();
        update.installer = None;
        let ui = present(
            Version::new(0, 2, 0),
            Edition::Installed,
            &Phase::Available(update),
        );
        assert!(ui.status.contains(TEXT_CHECKSUM_UNAVAILABLE));
        assert!(ui.status_error);
        assert_eq!(ui.action, ButtonAction::OpenRelease);
        assert_ne!(ui.action, ButtonAction::Install);
    }

    #[test]
    fn network_failure_has_no_tray_and_download_failure_keeps_the_update() {
        let mut session = UpdateSession::new();
        session.finish_check(CheckOutcome::Failed(TEXT_CHECK_FAILED), true);
        let ui = present(Version::new(0, 2, 0), Edition::Installed, session.phase());
        assert_eq!(ui.status, TEXT_CHECK_FAILED);
        assert!(ui.status_error);
        assert!(!ui.tray);
        assert_eq!(ui.action, ButtonAction::Check);

        session.finish_check(CheckOutcome::Available(sample()), false);
        session.begin_check();
        session.finish_check(CheckOutcome::Failed(TEXT_CHECK_FAILED), false);
        assert!(matches!(session.phase(), Phase::Available(_)));

        session.finish_check(CheckOutcome::Failed(TEXT_CHECK_FAILED), true);
        let ui = present(Version::new(0, 2, 0), Edition::Installed, session.phase());
        assert_eq!(ui.status, TEXT_CHECK_FAILED);
        assert!(ui.tray);
        assert_eq!(ui.action, ButtonAction::Install);

        session.fail(TEXT_DOWNLOAD_FAILED);
        let ui = present(Version::new(0, 2, 0), Edition::Installed, session.phase());
        assert_eq!(ui.status, TEXT_DOWNLOAD_FAILED);
        assert_eq!(ui.button, TEXT_BUTTON_INSTALL);
        assert!(ui.tray);
    }

    #[test]
    fn current_version_clears_the_indicator() {
        let mut session = UpdateSession::new();
        session.finish_check(CheckOutcome::Available(sample()), false);
        session.finish_check(CheckOutcome::Current, false);
        let ui = present(Version::new(0, 2, 0), Edition::Installed, session.phase());
        assert_eq!(ui.status, TEXT_UP_TO_DATE);
        assert!(!ui.tray);
        assert_eq!(ui.button, TEXT_BUTTON_CHECK);
    }

    #[test]
    fn download_status_uses_a_percentage_when_the_length_is_known() {
        assert_eq!(download_status(50, Some(200)), "正在下载 25%");
        assert_eq!(download_status(10, None), "正在下载");
        let mut session = UpdateSession::new();
        session.begin_download(sample());
        session.download_progress(1, Some(4));
        let ui = present(Version::new(0, 2, 0), Edition::Installed, session.phase());
        assert_eq!(ui.status, "正在下载 25%");
        assert!(!ui.button_enabled);
        assert_eq!(ui.action, ButtonAction::None);
    }
}
