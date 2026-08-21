//! 面向用户的应用错误类型。

use thiserror::Error;

/// 对平台错误进行业务分类，避免界面直接暴露 HRESULT 或 Win32 数值。
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum AppError {
    #[error("路径无效: {0}")]
    InvalidPath(String),
    #[error("文件不存在: {0}")]
    NotFound(String),
    #[error("访问被拒绝: {0}")]
    AccessDenied(String),
    #[error("进程已退出: {0}")]
    ProcessExited(u32),
    #[error("输入无效: {0}")]
    InvalidInput(String),
    #[error("当前系统不支持此操作: {0}")]
    Unsupported(String),
    #[error("Windows API 调用失败: {context} ({code})")]
    WindowsApi { context: String, code: String },
    #[error("资源管理器右键菜单操作失败: {0}")]
    ShellRegistrationFailed(String),
    #[error("单实例通信失败: {0}")]
    IpcFailed(String),
    #[error("配置读写失败: {0}")]
    Settings(String),
}

impl AppError {
    /// 返回适合常规用户界面展示的标题与可行动说明。
    pub fn user_message(&self) -> (&'static str, String) {
        match self {
            Self::InvalidPath(path) => ("路径无效", format!("无法识别路径：{path}")),
            Self::NotFound(path) => ("文件不存在", format!("找不到文件：{path}")),
            Self::AccessDenied(detail) => (
                "访问被拒绝",
                format!("可能需要管理员权限。详细信息：{detail}"),
            ),
            Self::ProcessExited(pid) => ("进程已退出", format!("PID {pid} 在查询期间已退出。")),
            Self::InvalidInput(detail) => ("输入无效", detail.clone()),
            Self::Unsupported(detail) => ("暂不支持", detail.clone()),
            Self::WindowsApi { context, code } => {
                ("系统查询失败", format!("{context}。详细信息：{code}"))
            }
            Self::ShellRegistrationFailed(detail) => ("右键菜单操作失败", detail.clone()),
            Self::IpcFailed(detail) => ("无法转发到已运行的程序", detail.clone()),
            Self::Settings(detail) => ("配置保存失败", detail.clone()),
        }
    }
}
