//! 工具模块向应用外壳声明意图的动作定义。

use std::path::PathBuf;

use crate::{core::invocation::ToolInvocation, model::ProcessSummary};

/// 工具 UI 仅返回动作，不直接引用其他工具或 Windows 平台对象。
#[derive(Clone, Debug)]
pub enum AppAction {
    NavigateTo(String),
    InvokeTool(ToolInvocation),
    QueryFileLocks { path: PathBuf },
    RefreshPorts,
    InspectProcess { pid: u32 },
    CopyText(String),
    OpenFileLocation(PathBuf),
    RequestTerminateProcess(ProcessSummary),
    TerminateProcess { pid: u32 },
    ToggleContextMenu { enabled: bool },
    ClearRecents,
}
