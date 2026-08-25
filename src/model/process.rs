//! 进程查询与进程树的领域模型。

use serde::{Deserialize, Serialize};

/// 用于列表、文件占用和端口行的轻量进程信息。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ProcessSummary {
    /// Windows 进程标识符。
    pub pid: u32,
    /// 从进程快照或 Restart Manager 读取的进程显示名称。
    pub name: String,
    /// 可执行文件的完整路径；权限不足或进程退出时为空。
    pub exe_path: Option<String>,
}

/// 进程详情页使用的完整进程信息。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ProcessInfo {
    /// 当前进程的 PID，来源于用户输入或跨工具调用。
    pub pid: u32,
    /// Toolhelp 快照中记录的父进程 PID；根进程或无法识别时为 `None`。
    pub parent_pid: Option<u32>,
    /// 进程映像名称，来源于 Toolhelp 快照。
    pub name: String,
    /// 可执行文件完整路径，来源于 QueryFullProcessImageNameW。
    pub exe_path: Option<String>,
    /// 命令行；当前版本不使用未公开 NT API，无法可靠读取时为 `None`。
    pub command_line: Option<String>,
    /// 本地创建时间字符串，来源于 GetProcessTimes；无权限时为 `None`。
    pub started_at: Option<String>,
    /// 从最早父进程到当前进程的完整链路，包含当前进程。
    pub parent_chain: Vec<ProcessSummary>,
    /// 当前快照中父 PID 等于本进程 PID 的直接子进程，不递归展开。
    pub children: Vec<ProcessSummary>,
}

/// 进程关系页使用的单个树节点。命令行不放在快照中，避免刷新时批量读取敏感信息。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ProcessTreeNode {
    /// Windows 进程标识符。
    pub pid: u32,
    /// Toolhelp 快照记录的父进程标识符；没有可见父进程时为 `None`。
    pub parent_pid: Option<u32>,
    /// 进程映像名称。
    pub name: String,
    /// 当前快照中可见的直接子进程 PID，按稳定顺序排列。
    pub children: Vec<u32>,
}

/// 一次一致的全量进程关系快照。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ProcessTreeSnapshot {
    /// 快照中的全部节点，按名称和 PID 稳定排序。
    pub nodes: Vec<ProcessTreeNode>,
    /// 根节点 PID；父进程不可见或为 0 的进程都列在这里。
    pub roots: Vec<u32>,
}

/// WMI 懒加载返回的命令行结果。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ProcessCommandLine {
    /// 进程 PID。
    pub pid: u32,
    /// WMI `Win32_Process.CommandLine`；字段缺失时为空。
    pub command_line: Option<String>,
}

impl ProcessInfo {
    pub fn summary(&self) -> ProcessSummary {
        ProcessSummary {
            pid: self.pid,
            name: self.name.clone(),
            exe_path: self.exe_path.clone(),
        }
    }
}
