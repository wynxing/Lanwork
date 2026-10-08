use std::thread;
use std::time::Duration;

use windows::Data::Xml::Dom::XmlDocument;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, SetCurrentProcessExplicitAppUserModelID,
};
use windows::core::HSTRING;

use crate::model::{AUMID, toast_xml};
use crate::util::{SpikeResult, pcwstr, wide, win_err};

pub struct ShowOutcome {
    pub results: Vec<SpikeResult<()>>,
    pub history_count: SpikeResult<u32>,
}

pub fn set_process_aumid() -> SpikeResult<()> {
    let name = wide(AUMID);
    unsafe { SetCurrentProcessExplicitAppUserModelID(pcwstr(&name)) }
        .map_err(|err| win_err("SetCurrentProcessExplicitAppUserModelID", err))
}

pub fn show_toasts(
    id: &str,
    heading: &str,
    body: &str,
    repeat: u32,
    tag: Option<&str>,
) -> ShowOutcome {
    let mut results = Vec::with_capacity(repeat as usize);
    for _ in 0..repeat {
        results.push(show_one(id, heading, body, tag));
    }
    thread::sleep(Duration::from_millis(400));
    let history_count = history_count();
    ShowOutcome {
        results,
        history_count,
    }
}

pub fn clear_history() -> SpikeResult<()> {
    let history = ToastNotificationManager::History()
        .map_err(|err| win_err("ToastNotificationManager::History", err))?;
    history
        .ClearWithId(&HSTRING::from(AUMID))
        .map_err(|err| win_err("清除该 AUMID 的通知历史", err))
}

pub fn notification_state_line() -> SpikeResult<String> {
    let state = unsafe { SHQueryUserNotificationState() }
        .map_err(|err| win_err("SHQueryUserNotificationState", err))?;
    let value = state.0;
    Ok(format!("{value} ({})", notification_state_name(value)))
}

fn history_count() -> SpikeResult<u32> {
    let history = ToastNotificationManager::History()
        .map_err(|err| win_err("ToastNotificationManager::History", err))?;
    let list = history
        .GetHistoryWithId(&HSTRING::from(AUMID))
        .map_err(|err| win_err("GetHistoryWithId", err))?;
    list.Size().map_err(|err| win_err("通知历史 Size", err))
}

fn show_one(id: &str, heading: &str, body: &str, tag: Option<&str>) -> SpikeResult<()> {
    let document = XmlDocument::new().map_err(|err| win_err("XmlDocument", err))?;
    let xml = HSTRING::from(toast_xml(id, heading, body));
    document
        .LoadXml(&xml)
        .map_err(|err| win_err("加载通知 XML", err))?;
    let toast = ToastNotification::CreateToastNotification(&document)
        .map_err(|err| win_err("CreateToastNotification", err))?;
    if let Some(tag) = tag {
        toast
            .SetTag(&HSTRING::from(tag))
            .map_err(|err| win_err("ToastNotification.Tag", err))?;
        toast
            .SetGroup(&HSTRING::from("lanwork-spike"))
            .map_err(|err| win_err("ToastNotification.Group", err))?;
    }
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID))
        .map_err(|err| win_err("CreateToastNotifierWithId", err))?;
    notifier
        .Show(&toast)
        .map_err(|err| win_err("ToastNotifier.Show", err))
}

fn notification_state_name(value: i32) -> &'static str {
    match value {
        1 => "QUNS_NOT_PRESENT",
        2 => "QUNS_BUSY",
        3 => "QUNS_RUNNING_D3D_FULL_SCREEN",
        4 => "QUNS_PRESENTATION_MODE",
        5 => "QUNS_ACCEPTS_NOTIFICATIONS",
        6 => "QUNS_QUIET_TIME",
        7 => "QUNS_APP",
        _ => "未知",
    }
}

pub fn format_outcome(outcome: &ShowOutcome) -> String {
    let mut lines = Vec::new();
    for (index, result) in outcome.results.iter().enumerate() {
        match result {
            Ok(()) => lines.push(format!("Show[{index}] = S_OK")),
            Err(err) => lines.push(format!("Show[{index}] = 失败 {err}")),
        }
    }
    match &outcome.history_count {
        Ok(count) => lines.push(format!("GetHistoryWithId 条数 = {count}")),
        Err(err) => lines.push(format!("GetHistoryWithId 失败: {err}")),
    }
    lines.join("\n")
}
