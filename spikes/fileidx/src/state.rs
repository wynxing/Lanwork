//! Everything 探测结果怎么归类。
//!
//! 数字来自随包 SDK 头文件，不在这里重新定义产品规则。
//! 「未安装」和「已安装但未运行」的 IPC 返回值相同，靠调用方是否看见客户端来区分。

use serde::Serialize;

/// Everything SDK 1.4，`Everything.h`。
pub const EVERYTHING_OK: u32 = 0;
pub const EVERYTHING_ERROR_IPC: u32 = 2;

/// Everything SDK3，`Everything3.h`。
pub const EVERYTHING3_OK: u32 = 0;
pub const EVERYTHING3_ERROR_IPC_PIPE_NOT_FOUND: u32 = 0xE000_0002;

/// architecture.md「搜索」：每次最多接收 50 条。
pub const MAX_RESULTS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeKind {
    NotRunning,
    NotReady,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineState {
    NotInstalled,
    InstalledNotRunning,
    RunningDbLoading,
    Ready,
    Unclassified,
}

pub fn classify_sdk14(is_db_loaded: bool, last_error: u32) -> ProbeKind {
    if is_db_loaded {
        ProbeKind::Ready
    } else if last_error == EVERYTHING_ERROR_IPC {
        ProbeKind::NotRunning
    } else if last_error == EVERYTHING_OK {
        ProbeKind::NotReady
    } else {
        ProbeKind::Failed
    }
}

pub fn classify_sdk3(connected: bool, is_db_loaded: bool, last_error: u32) -> ProbeKind {
    if !connected {
        if last_error == EVERYTHING3_ERROR_IPC_PIPE_NOT_FOUND {
            ProbeKind::NotRunning
        } else {
            ProbeKind::Failed
        }
    } else if is_db_loaded {
        ProbeKind::Ready
    } else {
        ProbeKind::NotReady
    }
}

/// `client_present` 为真表示这台机器上有 Everything 客户端（安装目录、服务，或调用方明确指出的便携副本）。
/// SDK 本身分不开「没装」和「装了但没运行」。
pub fn machine_state(client_present: bool, kind: ProbeKind) -> MachineState {
    match kind {
        ProbeKind::NotRunning if client_present => MachineState::InstalledNotRunning,
        ProbeKind::NotRunning => MachineState::NotInstalled,
        ProbeKind::NotReady => MachineState::RunningDbLoading,
        ProbeKind::Ready => MachineState::Ready,
        ProbeKind::Failed => MachineState::Unclassified,
    }
}

pub fn clamp_limit(limit: usize) -> usize {
    limit.clamp(1, MAX_RESULTS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdk14_separates_not_running_from_not_ready() {
        assert_eq!(
            classify_sdk14(false, EVERYTHING_ERROR_IPC),
            ProbeKind::NotRunning
        );
        assert_eq!(classify_sdk14(false, EVERYTHING_OK), ProbeKind::NotReady);
        assert_eq!(classify_sdk14(true, EVERYTHING_OK), ProbeKind::Ready);
        assert_eq!(classify_sdk14(false, 1), ProbeKind::Failed);
    }

    #[test]
    fn sdk3_pipe_missing_is_not_running() {
        assert_eq!(
            classify_sdk3(false, false, EVERYTHING3_ERROR_IPC_PIPE_NOT_FOUND),
            ProbeKind::NotRunning
        );
        assert_eq!(classify_sdk3(false, false, 1), ProbeKind::Failed);
        assert_eq!(
            classify_sdk3(true, false, EVERYTHING3_OK),
            ProbeKind::NotReady
        );
        assert_eq!(classify_sdk3(true, true, EVERYTHING3_OK), ProbeKind::Ready);
    }

    #[test]
    fn four_states_use_client_presence_only_for_ipc_failure() {
        assert_eq!(
            machine_state(false, ProbeKind::NotRunning),
            MachineState::NotInstalled
        );
        assert_eq!(
            machine_state(true, ProbeKind::NotRunning),
            MachineState::InstalledNotRunning
        );
        assert_eq!(
            machine_state(true, ProbeKind::NotReady),
            MachineState::RunningDbLoading
        );
        assert_eq!(machine_state(false, ProbeKind::Ready), MachineState::Ready);
    }

    #[test]
    fn result_limit_never_exceeds_50() {
        assert_eq!(clamp_limit(1), 1);
        assert_eq!(clamp_limit(50), 50);
        assert_eq!(clamp_limit(51), 50);
        assert_eq!(clamp_limit(0), 1);
    }
}
