//! 工具模块向应用外壳声明意图的动作定义。

use std::path::PathBuf;

use crate::{
    core::invocation::ToolInvocation,
    model::{CommunicationCommand, CommunicationConfig, CommunicationKind, ProcessSummary},
};

/// 工具 UI 仅返回动作，不直接引用其他工具或 Windows 平台对象。
#[derive(Clone, Debug)]
pub enum AppAction {
    NavigateTo(String),
    InvokeTool(ToolInvocation),
    ExportPortsCsv {
        content: String,
    },
    ExportCommunicationLog {
        content: String,
        file_name: String,
    },
    ExportText {
        content: String,
        file_name: String,
    },
    StartCommunication(CommunicationConfig),
    SendCommunication {
        kind: CommunicationKind,
        command: CommunicationCommand,
    },
    StopCommunication(CommunicationKind),
    RefreshSerialPorts,
    RunDns {
        host: String,
        record_type: crate::model::DnsRecordType,
        bypass_cache: bool,
    },
    StopDns,
    RunPing {
        host: String,
        config: crate::model::PingConfig,
    },
    StopPing,
    RunTcpProbe {
        host: String,
        port: u16,
        config: crate::model::TcpProbeConfig,
    },
    StopTcpProbe,
    RunMtr {
        host: String,
        config: crate::model::MtrConfig,
    },
    StopMtr,
    QueryFileLocks {
        path: PathBuf,
    },
    RefreshPorts,
    LoadPortProcessDetails {
        pid: u32,
    },
    LoadProcessTree,
    InspectProcess {
        pid: u32,
    },
    InspectProcessCommandLine {
        pid: u32,
    },
    CopyText(String),
    OpenFileLocation(PathBuf),
    OpenDirectory(PathBuf),
    RequestTerminateProcess(ProcessSummary),
    TerminateProcess {
        pid: u32,
    },
    ToggleContextMenu {
        enabled: bool,
    },
    ToggleFavorite {
        tool_id: String,
    },
    ClearRecents,
}
