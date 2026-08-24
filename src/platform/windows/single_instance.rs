//! 使用命名 Mutex 与命名管道转发启动调用。
//! 次实例只发送序列化的 ToolInvocation 后退出，主实例在 UI 线程安全地轮询接收队列。

use std::{sync::mpsc, thread, time::Duration};

use windows::{
    Win32::{
        Foundation::{
            CloseHandle, ERROR_ALREADY_EXISTS, ERROR_PIPE_CONNECTED, GetLastError, HANDLE,
        },
        Storage::FileSystem::{
            CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE, FILE_SHARE_NONE, OPEN_EXISTING,
            PIPE_ACCESS_INBOUND, ReadFile, WriteFile,
        },
        System::{
            Pipes::{
                ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_MESSAGE, PIPE_TYPE_MESSAGE,
                PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
            },
            Threading::CreateMutexW,
        },
    },
    core::PCWSTR,
};

use crate::{core::invocation::ToolInvocation, model::AppError, platform::windows::wide};

// 升级命名版本，避免仍在运行但窗口不可达的旧 V0_1 实例阻断本轮新 EXE 的冷启动。
const MUTEX_NAME: &str = "Local\\WindowsToolbox.V0_2";
const PIPE_NAME: &str = r"\\.\pipe\WindowsToolbox.V0_2.Invocation";
const PIPE_BUFFER_SIZE: u32 = 16_384;

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

/// 单实例协调入口：若已有实例，尝试可靠转发调用；若不存在，则启动监听器。
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
            // 无命令行工具调用时也必须唤醒主窗口，避免普通双击被静默当作次实例退出。
            let invocation = invocation.unwrap_or_else(ToolInvocation::activate);
            send_to_primary(&invocation)?;
            drop(mutex);
            return Ok(InstanceRole::Secondary);
        }

        let (sender, receiver) = mpsc::channel();
        start_pipe_listener(sender);
        Ok(InstanceRole::Primary(PrimaryInstance {
            _mutex: mutex,
            receiver: Some(receiver),
        }))
    }
}

struct WinHandle(HANDLE);

impl Drop for WinHandle {
    fn drop(&mut self) {
        // Safety: 包装器只接收成功的 Windows HANDLE，析构时关闭不影响其他实例持有的同名对象。
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn start_pipe_listener(sender: mpsc::Sender<ToolInvocation>) {
    thread::spawn(move || {
        loop {
            if let Err(error) = receive_one(&sender) {
                tracing::warn!("命名管道监听暂时失败: {error}");
                thread::sleep(Duration::from_millis(150));
            }
        }
    });
}

fn receive_one(sender: &mpsc::Sender<ToolInvocation>) -> Result<(), AppError> {
    let pipe_name = wide::to_wide(PIPE_NAME);
    // Safety: pipe_name 是 NUL 结尾 UTF-16；PIPE 缓冲区大小固定且输出句柄由 WinHandle 取得所有权。
    let pipe = unsafe {
        CreateNamedPipeW(
            PCWSTR(pipe_name.as_ptr()),
            PIPE_ACCESS_INBOUND,
            PIPE_TYPE_MESSAGE | PIPE_READMODE_MESSAGE | PIPE_WAIT,
            PIPE_UNLIMITED_INSTANCES,
            0,
            PIPE_BUFFER_SIZE,
            0,
            None,
        )
    };
    if pipe.is_invalid() {
        return Err(AppError::IpcFailed(format!(
            "无法创建命名管道：{}",
            // Safety: 读取失败 API 紧邻的线程局部错误码，不解引用原始指针。
            unsafe { GetLastError() }.0
        )));
    }
    let pipe = WinHandle(pipe);

    // Safety: pipe.0 是 CreateNamedPipeW 成功返回且仍由 WinHandle 持有的同步命名管道句柄。
    let connected = unsafe { ConnectNamedPipe(pipe.0, None) };
    // Safety: 读取紧邻 ConnectNamedPipe 的线程局部错误码，不解引用原始指针。
    let connect_error = unsafe { GetLastError() };
    if connected.is_err() && connect_error != ERROR_PIPE_CONNECTED {
        return Err(AppError::IpcFailed("无法接受命名管道连接。".into()));
    }

    let mut bytes = vec![0_u8; PIPE_BUFFER_SIZE as usize];
    let mut read = 0_u32;
    // Safety: bytes 是 PIPE_BUFFER_SIZE 长度的可写缓冲区，read 是有效输出地址，pipe.0 仍由 WinHandle 持有。
    unsafe { ReadFile(pipe.0, Some(&mut bytes), Some(&mut read), None) }
        .map_err(|error| AppError::IpcFailed(format!("无法读取命名管道调用：{error}")))?;
    let invocation = serde_json::from_slice::<ToolInvocation>(&bytes[..read as usize])
        .map_err(|error| AppError::IpcFailed(format!("调用数据格式无效：{error}")))?;
    let _ = sender.send(invocation);
    Ok(())
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

    for _ in 0..12 {
        // Safety: pipe_name 是 NUL 结尾 UTF-16，次实例只申请写入权限，成功句柄随后交由 WinHandle 管理。
        let opened = unsafe {
            CreateFileW(
                PCWSTR(pipe_name.as_ptr()),
                FILE_GENERIC_WRITE.0,
                FILE_SHARE_NONE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        };
        if let Ok(handle) = opened {
            let pipe = WinHandle(handle);
            let mut written = 0_u32;
            // Safety: bytes 在同步写入完成前保持有效，written 是有效输出地址，pipe.0 是独占有效句柄。
            unsafe { WriteFile(pipe.0, Some(&bytes), Some(&mut written), None) }
                .map_err(|error| AppError::IpcFailed(format!("无法写入命名管道调用：{error}")))?;
            if written as usize == bytes.len() {
                return Ok(());
            }
            return Err(AppError::IpcFailed("命名管道未完整写入调用数据。".into()));
        }
        // 主实例创建 Mutex 与监听管道之间存在极短窗口；后台重试不阻塞任何 GUI 线程。
        thread::sleep(Duration::from_millis(80));
    }
    Err(AppError::IpcFailed(
        "已检测到主实例，但其命名管道未在预期时间内就绪。".into(),
    ))
}
