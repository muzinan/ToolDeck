//! 使用命名 Mutex 与双向命名管道转发启动调用。
//! 次实例仅在收到主实例 Accepted 回执后正常退出，主实例在 UI 线程安全地消费调用队列。

use std::{sync::mpsc, thread, time::Duration};

use serde::{Deserialize, Serialize};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, GetLastError, HANDLE,
        },
        Storage::FileSystem::{PIPE_ACCESS_DUPLEX, ReadFile, WriteFile},
        System::{
            Pipes::{
                CallNamedPipeW, ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_MESSAGE,
                PIPE_TYPE_MESSAGE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
            },
            Threading::CreateMutexW,
        },
    },
    core::PCWSTR,
};

use crate::{core::invocation::ToolInvocation, model::AppError, platform::windows::wide};

// 双向回执改变了管道协议，因此升级命名版本，避免与仍在运行的旧版实例错误互通。
const MUTEX_NAME: &str = "Local\\WindowsToolbox.V0_3";
const PIPE_NAME: &str = r"\\.\pipe\WindowsToolbox.V0_3.Invocation";
const PIPE_BUFFER_SIZE: u32 = 16_384;
const SEND_RETRY_COUNT: usize = 12;
const SEND_RETRY_DELAY: Duration = Duration::from_millis(80);
const EXCHANGE_TIMEOUT_MILLIS: u32 = 1_200;

#[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
enum IpcReply {
    Accepted,
    Rejected { reason: String },
}

pub enum InstanceRole {
    Primary(PrimaryInstance),
    Secondary,
}

/// 主实例持有 Mutex 直到 GUI 退出，并保存来自命名管道监听线程的调用队列。
pub struct PrimaryInstance {
    _mutex: WinHandle,
    receiver: Option<mpsc::Receiver<ToolInvocation>>,
}

impl PrimaryInstance {
    pub fn take_receiver(&mut self) -> mpsc::Receiver<ToolInvocation> {
        self.receiver
            .take()
            .expect("主实例调用接收端只能被 GUI 外壳取得一次")
    }
}

/// 单实例协调入口：若已有实例，可靠转发调用并等待回执；若不存在，先同步创建首个管道再返回。
pub struct SingleInstance;

impl SingleInstance {
    pub fn acquire(invocation: Option<ToolInvocation>) -> Result<InstanceRole, AppError> {
        let mutex_name = wide::to_wide(MUTEX_NAME);
        // Safety: 命名 Mutex 的名称缓冲区是 NUL 结尾 UTF-16；返回句柄由 WinHandle 独占管理。
        let mutex = unsafe { CreateMutexW(None, false, PCWSTR(mutex_name.as_ptr())) }
            .map(WinHandle)
            .map_err(|error| AppError::IpcFailed(format!("无法创建实例互斥体：{error}")))?;

        // Safety: 读取紧邻 CreateMutexW 的线程局部错误码，不解引用任何原始指针。
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            let invocation = invocation.unwrap_or_else(ToolInvocation::activate);
            let result = send_to_primary(&invocation);
            // 失败路径也显式释放次实例 Mutex 句柄，避免错误提示期间延长旧命名对象生命周期。
            drop(mutex);
            result?;
            return Ok(InstanceRole::Secondary);
        }

        // 首个管道必须在主实例进入 GUI 初始化前同步创建，消除 Mutex 已存在但管道尚未就绪的启动窗口。
        let first_pipe = create_pipe()?;
        let (sender, receiver) = mpsc::channel();
        start_pipe_listener(first_pipe, sender)?;
        Ok(InstanceRole::Primary(PrimaryInstance {
            _mutex: mutex,
            receiver: Some(receiver),
        }))
    }
}

struct WinHandle(HANDLE);

// SAFETY: Windows 内核句柄可在线程间转移；WinHandle 保持唯一所有权且只在最终持有线程关闭一次。
unsafe impl Send for WinHandle {}

impl Drop for WinHandle {
    fn drop(&mut self) {
        // Safety: 包装器只接收成功的 Windows HANDLE，析构时关闭不影响其他实例持有的同名对象。
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn create_pipe() -> Result<WinHandle, AppError> {
    let pipe_name = wide::to_wide(PIPE_NAME);
    // Safety: pipe_name 是 NUL 结尾 UTF-16；缓冲区大小固定且输出句柄由 WinHandle 取得所有权。
    let pipe = unsafe {
        CreateNamedPipeW(
            PCWSTR(pipe_name.as_ptr()),
            PIPE_ACCESS_DUPLEX,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            PIPE_BUFFER_SIZE,
            PIPE_BUFFER_SIZE,
            0,
            None,
        )
    };
    if pipe.is_invalid() {
        // Safety: 读取紧邻 CreateNamedPipeW 失败的线程局部错误码。
        let code = unsafe { GetLastError() }.0;
        Err(AppError::IpcFailed(format!(
            "无法创建命名管道，Win32 error {code}"
        )))
    } else {
        Ok(WinHandle(pipe))
    }
}

fn start_pipe_listener(
    first_pipe: WinHandle,
    sender: mpsc::Sender<ToolInvocation>,
) -> Result<(), AppError> {
    thread::Builder::new()
        .name("toolbox-ipc-listener".into())
        .spawn(move || {
            let mut next_pipe = Some(first_pipe);
            loop {
                let pipe = match next_pipe.take() {
                    Some(pipe) => pipe,
                    None => match create_pipe() {
                        Ok(pipe) => pipe,
                        Err(error) => {
                            tracing::warn!(
                                error_kind = error.diagnostic_label(),
                                "命名管道创建暂时失败"
                            );
                            thread::sleep(Duration::from_millis(150));
                            continue;
                        }
                    },
                };
                if let Err(error) = receive_one(pipe, &sender) {
                    tracing::warn!(
                        error_kind = error.diagnostic_label(),
                        "命名管道监听暂时失败"
                    );
                    thread::sleep(Duration::from_millis(150));
                }
            }
        })
        .map(|_| ())
        .map_err(|error| AppError::IpcFailed(format!("无法创建命名管道监听线程：{error}")))
}

fn receive_one(pipe: WinHandle, sender: &mpsc::Sender<ToolInvocation>) -> Result<(), AppError> {
    // Safety: pipe.0 是 CreateNamedPipeW 成功返回且仍由 WinHandle 持有的同步命名管道句柄。
    let connected = unsafe { ConnectNamedPipe(pipe.0, None) };
    // Safety: 读取紧邻 ConnectNamedPipe 的线程局部错误码。
    let connect_error = unsafe { GetLastError() };
    if connected.is_err() && connect_error != ERROR_PIPE_CONNECTED {
        return Err(AppError::IpcFailed(format!(
            "无法接受命名管道连接，Win32 error {}",
            connect_error.0
        )));
    }

    let mut bytes = vec![0_u8; PIPE_BUFFER_SIZE as usize];
    let mut read = 0_u32;
    // Safety: bytes 是固定长度可写缓冲区，read 是有效输出地址，pipe.0 仍由 WinHandle 持有。
    unsafe { ReadFile(pipe.0, Some(&mut bytes), Some(&mut read), None) }
        .map_err(|error| AppError::IpcFailed(format!("无法读取命名管道调用：{error}")))?;

    let reply = reply_for_payload(&bytes[..read as usize], sender);
    write_reply(pipe.0, &reply)
}

fn reply_for_payload(bytes: &[u8], sender: &mpsc::Sender<ToolInvocation>) -> IpcReply {
    match serde_json::from_slice::<ToolInvocation>(bytes) {
        Ok(invocation) => match sender.send(invocation) {
            Ok(()) => IpcReply::Accepted,
            Err(_) => IpcReply::Rejected {
                reason: "主窗口已关闭，无法接收调用。".into(),
            },
        },
        Err(error) => IpcReply::Rejected {
            reason: format!("调用数据格式无效：{error}"),
        },
    }
}

fn write_reply(pipe: HANDLE, reply: &IpcReply) -> Result<(), AppError> {
    let bytes = serde_json::to_vec(reply)
        .map_err(|error| AppError::IpcFailed(format!("无法序列化命名管道回执：{error}")))?;
    let mut written = 0_u32;
    // Safety: bytes 在同步写入完成前保持有效，written 是有效输出地址，pipe 是有效连接句柄。
    unsafe { WriteFile(pipe, Some(&bytes), Some(&mut written), None) }
        .map_err(|error| AppError::IpcFailed(format!("无法写入命名管道回执：{error}")))?;
    if written as usize == bytes.len() {
        Ok(())
    } else {
        Err(AppError::IpcFailed("命名管道未完整写入回执。".into()))
    }
}

fn send_to_primary(invocation: &ToolInvocation) -> Result<(), AppError> {
    let bytes = serde_json::to_vec(invocation)
        .map_err(|error| AppError::IpcFailed(format!("无法序列化工具调用：{error}")))?;
    if bytes.len() > PIPE_BUFFER_SIZE as usize {
        return Err(AppError::IpcFailed(
            "工具调用路径过长，无法通过命名管道传递。".into(),
        ));
    }
    let pipe_name = wide::to_wide(PIPE_NAME);
    let mut last_error = "未执行命名管道连接".to_owned();

    for _ in 0..SEND_RETRY_COUNT {
        let mut reply_bytes = vec![0_u8; PIPE_BUFFER_SIZE as usize];
        let mut read = 0_u32;
        // Safety: 名称为 NUL 结尾 UTF-16；输入输出缓冲区在同步调用期间有效，超时限制了等待主实例回执的时间。
        let exchanged = unsafe {
            CallNamedPipeW(
                PCWSTR(pipe_name.as_ptr()),
                Some(bytes.as_ptr().cast()),
                bytes.len() as u32,
                Some(reply_bytes.as_mut_ptr().cast()),
                reply_bytes.len() as u32,
                &mut read,
                EXCHANGE_TIMEOUT_MILLIS,
            )
        };
        if exchanged.as_bool() {
            return parse_reply(&reply_bytes[..read as usize]);
        }
        // Safety: 读取紧邻 CallNamedPipeW 失败的线程局部错误码，作为有限重试后的最终诊断依据。
        last_error = unsafe { GetLastError() }.0.to_string();
        thread::sleep(SEND_RETRY_DELAY);
    }
    Err(AppError::IpcFailed(format!(
        "已检测到主实例，但命名管道在有限重试后仍不可用；最终 Win32 错误：{last_error}"
    )))
}

fn parse_reply(reply_bytes: &[u8]) -> Result<(), AppError> {
    let reply = serde_json::from_slice::<IpcReply>(reply_bytes)
        .map_err(|error| AppError::IpcFailed(format!("主实例回执格式无效：{error}")))?;
    match reply {
        IpcReply::Accepted => Ok(()),
        IpcReply::Rejected { reason } => Err(AppError::IpcFailed(format!(
            "主实例拒绝了本次调用：{reason}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use crate::core::invocation::ToolInvocation;

    use super::{IpcReply, parse_reply, reply_for_payload};

    #[test]
    fn ipc_reply_round_trips() {
        for reply in [
            IpcReply::Accepted,
            IpcReply::Rejected {
                reason: "测试拒绝".into(),
            },
        ] {
            let bytes = serde_json::to_vec(&reply).unwrap();
            assert_eq!(serde_json::from_slice::<IpcReply>(&bytes).unwrap(), reply);
        }
    }

    #[test]
    fn valid_payload_is_forwarded_and_accepted() {
        let (sender, receiver) = mpsc::channel();
        let invocation = ToolInvocation::activate();
        let bytes = serde_json::to_vec(&invocation).unwrap();
        assert_eq!(reply_for_payload(&bytes, &sender), IpcReply::Accepted);
        assert_eq!(receiver.recv().unwrap(), invocation);
        assert!(parse_reply(&serde_json::to_vec(&IpcReply::Accepted).unwrap()).is_ok());
    }

    #[test]
    fn invalid_or_unroutable_payload_is_rejected() {
        let (sender, receiver) = mpsc::channel();
        assert!(matches!(
            reply_for_payload(b"not-json", &sender),
            IpcReply::Rejected { .. }
        ));
        drop(receiver);
        let invocation = serde_json::to_vec(&ToolInvocation::activate()).unwrap();
        let reply = reply_for_payload(&invocation, &sender);
        assert!(matches!(reply, IpcReply::Rejected { .. }));
        assert!(parse_reply(&serde_json::to_vec(&reply).unwrap()).is_err());
    }
}
