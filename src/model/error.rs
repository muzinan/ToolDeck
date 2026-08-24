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
    /// 为后续工具和平台能力扩展保留的统一错误类型；当前版本尚无对应失败路径。
    #[allow(dead_code)]
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
    #[error("后台任务暂时不可用: {0}")]
    WorkerBusy(String),
    #[error("后台任务队列已满: {0}")]
    QueueFull(String),
    #[error("名称解析失败: {0}")]
    NameResolution(String),
    #[allow(dead_code)]
    #[error("网络请求超时: {0}")]
    Timeout(String),
    #[allow(dead_code)]
    #[error("连接被拒绝: {0}")]
    ConnectionRefused(String),
    #[allow(dead_code)]
    #[error("网络不可达: {0}")]
    NetworkUnreachable(String),
}

impl AppError {
    /// 返回不含路径、查询值、PID 或端口的稳定诊断分类，供运行日志安全记录。
    pub fn diagnostic_label(&self) -> &'static str {
        match self {
            Self::InvalidPath(_) => "invalid-path",
            Self::NotFound(_) => "not-found",
            Self::AccessDenied(_) => "access-denied",
            Self::ProcessExited(_) => "process-exited",
            Self::InvalidInput(_) => "invalid-input",
            Self::Unsupported(_) => "unsupported",
            Self::WindowsApi { .. } => "windows-api",
            Self::ShellRegistrationFailed(_) => "shell-registration",
            Self::IpcFailed(_) => "ipc",
            Self::Settings(_) => "settings",
            Self::WorkerBusy(_) => "worker-busy",
            Self::QueueFull(_) => "queue-full",
            Self::NameResolution(_) => "name-resolution",
            Self::Timeout(_) => "timeout",
            Self::ConnectionRefused(_) => "connection-refused",
            Self::NetworkUnreachable(_) => "network-unreachable",
        }
    }

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
            Self::WorkerBusy(detail) => ("后台任务繁忙", detail.clone()),
            Self::QueueFull(detail) => ("后台任务队列已满", detail.clone()),
            Self::NameResolution(detail) => ("名称解析失败", detail.clone()),
            Self::Timeout(detail) => ("请求超时", detail.clone()),
            Self::ConnectionRefused(detail) => ("连接被拒绝", detail.clone()),
            Self::NetworkUnreachable(detail) => ("网络不可达", detail.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AppError;

    #[test]
    fn diagnostic_labels_never_include_sensitive_error_values() {
        let errors = [
            AppError::InvalidPath(r"C:\Users\张三\secret.txt".into()),
            AppError::InvalidInput("端口 54321 无效".into()),
            AppError::ProcessExited(98_765),
            AppError::WindowsApi {
                context: r"无法读取 C:\secret".into(),
                code: "Win32 5".into(),
            },
        ];
        for error in errors {
            let label = error.diagnostic_label();
            assert!(!label.contains("secret"));
            assert!(!label.contains("54321"));
            assert!(!label.contains("98765"));
            assert!(!label.contains("张三"));
        }
    }
}
