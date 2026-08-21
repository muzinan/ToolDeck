//! 后台任务协议与执行器。
//! UI 线程只提交请求和消费结果，所有潜在阻塞的 Windows API 调用都在工作线程执行。

use std::{path::PathBuf, sync::mpsc, thread};

use crate::{
    model::{AppError, FileLockResult, NetworkEndpoint, ProcessInfo},
    platform::windows,
};

#[derive(Clone, Debug)]
pub enum TaskRequest {
    FileLocks { path: PathBuf },
    Ports,
    Process { pid: u32 },
    TerminateProcess { pid: u32 },
}

#[derive(Clone, Debug)]
pub enum TaskResult {
    FileLocks(Result<FileLockResult, AppError>),
    Ports(Result<Vec<NetworkEndpoint>, AppError>),
    Process(Result<ProcessInfo, AppError>),
    ProcessTerminated(Result<u32, AppError>),
}

impl TaskResult {
    pub fn target_tool_id(&self) -> &'static str {
        match self {
            Self::FileLocks(_) => "file-lock",
            Self::Ports(_) => "port-inspector",
            Self::Process(_) | Self::ProcessTerminated(_) => "process-inspector",
        }
    }
}

/// 小型任务派发器；调用方通过“同一工具同一时刻仅一项请求”策略形成背压。
#[derive(Clone)]
pub struct TaskDispatcher {
    sender: mpsc::Sender<TaskResult>,
}

impl TaskDispatcher {
    pub fn new() -> (Self, mpsc::Receiver<TaskResult>) {
        let (sender, receiver) = mpsc::channel();
        (Self { sender }, receiver)
    }

    pub fn dispatch(&self, request: TaskRequest) {
        let sender = self.sender.clone();
        thread::spawn(move || {
            let result = match request {
                TaskRequest::FileLocks { path } => {
                    TaskResult::FileLocks(windows::query_file_locks(&path))
                }
                TaskRequest::Ports => TaskResult::Ports(windows::query_network_endpoints()),
                TaskRequest::Process { pid } => TaskResult::Process(windows::query_process(pid)),
                TaskRequest::TerminateProcess { pid } => {
                    TaskResult::ProcessTerminated(windows::terminate_process(pid).map(|()| pid))
                }
            };

            // 接收端关闭表示 GUI 正在退出，此时静默丢弃已经完成的结果。
            let _ = sender.send(result);
        });
    }
}
