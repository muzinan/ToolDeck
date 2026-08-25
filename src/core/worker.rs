//! 后台任务协议与固定线程执行器。
//! UI 线程只提交有界请求并消费事件；所有潜在阻塞的 Windows API 调用均由固定工作线程执行。

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
};

use crate::{
    model::{
        AppError, DnsRecordType, DnsResult, FileLockResult, MtrConfig, MtrProgress, MtrResult,
        NetworkEndpoint, PingAddressFamily, PingSummary, ProcessCommandLine, ProcessInfo,
        ProcessTreeSnapshot, TcpProbeResult,
    },
    platform::windows,
};

pub type RequestId = u64;

pub const WORKER_THREAD_COUNT: usize = 4;
pub const REQUEST_QUEUE_CAPACITY: usize = 32;

type CancellationMap = HashMap<&'static str, (RequestId, Arc<AtomicBool>)>;

#[derive(Clone, Debug)]
pub enum TaskRequest {
    FileLocks {
        path: PathBuf,
    },
    Ports,
    Process {
        pid: u32,
    },
    ProcessTree,
    ProcessCommandLine {
        pid: u32,
    },
    TerminateProcess {
        pid: u32,
    },
    DnsLookup {
        host: String,
        record_type: DnsRecordType,
        bypass_cache: bool,
    },
    Ping {
        host: String,
        count: u32,
        timeout_ms: u32,
        payload_size: u16,
        family: PingAddressFamily,
    },
    TcpProbe {
        host: String,
        port: u16,
        timeout_ms: u32,
    },
    Mtr {
        host: String,
        config: MtrConfig,
    },
}

impl TaskRequest {
    pub fn target_tool_id(&self) -> &'static str {
        match self {
            Self::FileLocks { .. } => "file-lock",
            Self::Ports => "port-inspector",
            Self::Process { .. }
            | Self::ProcessTree
            | Self::ProcessCommandLine { .. }
            | Self::TerminateProcess { .. } => "process-inspector",
            Self::DnsLookup { .. } => "dns-lookup",
            Self::Ping { .. } => "ping",
            Self::TcpProbe { .. } => "tcp-probe",
            Self::Mtr { .. } => "mtr",
        }
    }
}

/// 请求信封将单调递增标识与具体任务绑定，供 UI 丢弃同工具的旧结果。
#[derive(Clone, Debug)]
pub struct TaskRequestEnvelope {
    pub request_id: RequestId,
    pub request: TaskRequest,
    pub cancel: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
pub enum TaskResult {
    FileLocks(Result<FileLockResult, AppError>),
    Ports(Result<Vec<NetworkEndpoint>, AppError>),
    Process(Result<ProcessInfo, AppError>),
    ProcessTree(Result<ProcessTreeSnapshot, AppError>),
    ProcessCommandLine {
        pid: u32,
        result: Result<ProcessCommandLine, AppError>,
    },
    ProcessTerminated(Result<u32, AppError>),
    Dns(Result<DnsResult, AppError>),
    Ping(Result<PingSummary, AppError>),
    TcpProbe(Result<TcpProbeResult, AppError>),
    Mtr(Result<MtrResult, AppError>),
}

impl TaskResult {
    pub fn error(&self) -> Option<&AppError> {
        match self {
            Self::FileLocks(Err(error))
            | Self::Ports(Err(error))
            | Self::Process(Err(error))
            | Self::ProcessTree(Err(error))
            | Self::ProcessTerminated(Err(error)) => Some(error),
            Self::ProcessCommandLine {
                result: Err(error), ..
            } => Some(error),
            Self::Dns(Err(error))
            | Self::Ping(Err(error))
            | Self::TcpProbe(Err(error))
            | Self::Mtr(Err(error)) => Some(error),
            Self::FileLocks(Ok(_))
            | Self::Ports(Ok(_))
            | Self::Process(Ok(_))
            | Self::ProcessTree(Ok(_))
            | Self::ProcessTerminated(Ok(_)) => None,
            Self::ProcessCommandLine { result: Ok(_), .. } => None,
            Self::Dns(Ok(_)) | Self::Ping(Ok(_)) | Self::TcpProbe(Ok(_)) | Self::Mtr(Ok(_)) => None,
        }
    }
}

/// 后台事件预留进度与终态两种形态，后续 Ping 等长任务无需修改 UI 消息边界。
#[derive(Clone, Debug)]
pub enum TaskEvent {
    #[allow(dead_code)]
    Progress {
        message: String,
        completed: Option<u64>,
        total: Option<u64>,
    },
    Finished(TaskResult),
    MtrProgress(MtrProgress),
}

/// 事件信封携带请求与目标工具标识，UI 可在进入工具模块前完成最新请求判定。
#[derive(Clone, Debug)]
pub struct TaskEventEnvelope {
    pub request_id: RequestId,
    pub target_tool_id: &'static str,
    pub event: TaskEvent,
}

/// 固定四线程执行器使用容量 32 的有界队列，避免短时间重复操作无限创建线程或堆积任务。
#[derive(Clone)]
pub struct TaskDispatcher {
    sender: mpsc::SyncSender<TaskRequestEnvelope>,
    next_request_id: Arc<AtomicU64>,
    cancellations: Arc<Mutex<CancellationMap>>,
}

impl TaskDispatcher {
    pub fn new() -> Result<(Self, mpsc::Receiver<TaskEventEnvelope>), AppError> {
        let (request_sender, request_receiver) = mpsc::sync_channel(REQUEST_QUEUE_CAPACITY);
        let (event_sender, event_receiver) = mpsc::channel();
        let request_receiver = Arc::new(Mutex::new(request_receiver));

        for index in 0..WORKER_THREAD_COUNT {
            let receiver = Arc::clone(&request_receiver);
            let sender = event_sender.clone();
            thread::Builder::new()
                .name(format!("toolbox-worker-{index}"))
                .spawn(move || worker_loop(receiver, sender))
                .map_err(|error| {
                    AppError::WorkerBusy(format!("无法创建后台工作线程 {index}：{error}"))
                })?;
        }
        drop(event_sender);

        Ok((
            Self {
                sender: request_sender,
                next_request_id: Arc::new(AtomicU64::new(1)),
                cancellations: Arc::new(Mutex::new(HashMap::new())),
            },
            event_receiver,
        ))
    }

    pub fn dispatch(&self, request: TaskRequest) -> Result<RequestId, AppError> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let target_tool_id = request.target_tool_id();
        let cancel = Arc::new(AtomicBool::new(false));
        let envelope = TaskRequestEnvelope {
            request_id,
            request,
            cancel: Arc::clone(&cancel),
        };
        self.sender
            .try_send(envelope)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    AppError::QueueFull("后台任务队列已满，请稍后重试。".into())
                }
                mpsc::TrySendError::Disconnected(_) => {
                    AppError::WorkerBusy("后台工作线程已停止，请重新启动程序。".into())
                }
            })?;
        if let Ok(mut cancellations) = self.cancellations.lock() {
            cancellations.insert(target_tool_id, (request_id, cancel));
        }
        Ok(request_id)
    }

    /// 请求长任务在下一次探测边界停止。
    pub fn cancel(&self, request_id: RequestId) -> bool {
        let Ok(cancellations) = self.cancellations.lock() else {
            return false;
        };
        let Some((_, token)) = cancellations
            .values()
            .find(|(candidate, _)| *candidate == request_id)
        else {
            return false;
        };
        token.store(true, Ordering::Release);
        true
    }
}

#[cfg(test)]
fn try_enqueue(
    sender: &mpsc::SyncSender<TaskRequestEnvelope>,
    next_request_id: &AtomicU64,
    request: TaskRequest,
) -> Result<RequestId, AppError> {
    let request_id = next_request_id.fetch_add(1, Ordering::Relaxed);
    let envelope = TaskRequestEnvelope {
        request_id,
        request,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    sender.try_send(envelope).map_err(|error| match error {
        mpsc::TrySendError::Full(_) => AppError::QueueFull("后台任务队列已满，请稍后重试。".into()),
        mpsc::TrySendError::Disconnected(_) => {
            AppError::WorkerBusy("后台工作线程已停止，请重新启动程序。".into())
        }
    })?;
    Ok(request_id)
}

fn worker_loop(
    receiver: Arc<Mutex<mpsc::Receiver<TaskRequestEnvelope>>>,
    sender: mpsc::Sender<TaskEventEnvelope>,
) {
    loop {
        let envelope = {
            let Ok(receiver) = receiver.lock() else {
                return;
            };
            let Ok(envelope) = receiver.recv() else {
                return;
            };
            envelope
        };
        let target_tool_id = envelope.request.target_tool_id();
        let result = execute_with_progress(
            envelope.request,
            &sender,
            envelope.request_id,
            target_tool_id,
            &envelope.cancel,
        );
        for event in ordinary_task_events(envelope.request_id, target_tool_id, result) {
            // 接收端关闭表示 GUI 正在退出，此时结束当前工作线程，不再消费后续请求。
            if sender.send(event).is_err() {
                return;
            }
        }
    }
}

fn ordinary_task_events(
    request_id: RequestId,
    target_tool_id: &'static str,
    result: TaskResult,
) -> [TaskEventEnvelope; 1] {
    [TaskEventEnvelope {
        request_id,
        target_tool_id,
        event: TaskEvent::Finished(result),
    }]
}

fn execute(request: TaskRequest) -> TaskResult {
    match request {
        TaskRequest::FileLocks { path } => TaskResult::FileLocks(windows::query_file_locks(&path)),
        TaskRequest::Ports => TaskResult::Ports(windows::query_network_endpoints()),
        TaskRequest::Process { pid } => TaskResult::Process(windows::query_process(pid)),
        TaskRequest::ProcessTree => TaskResult::ProcessTree(windows::query_process_tree()),
        TaskRequest::ProcessCommandLine { pid } => TaskResult::ProcessCommandLine {
            pid,
            result: windows::query_process_command_line(pid),
        },
        TaskRequest::TerminateProcess { pid } => {
            TaskResult::ProcessTerminated(windows::terminate_process(pid).map(|()| pid))
        }
        TaskRequest::DnsLookup {
            host,
            record_type,
            bypass_cache,
        } => TaskResult::Dns(windows::query_dns(&host, record_type, bypass_cache)),
        TaskRequest::Ping {
            host,
            count,
            timeout_ms,
            payload_size,
            family,
        } => TaskResult::Ping(windows::ping_host(
            &host,
            count,
            timeout_ms,
            payload_size,
            family,
        )),
        TaskRequest::TcpProbe {
            host,
            port,
            timeout_ms,
        } => TaskResult::TcpProbe(windows::tcp_probe(&host, port, timeout_ms)),
        TaskRequest::Mtr { host, config } => TaskResult::Mtr(windows::mtr_host(&host, &config)),
    }
}

fn execute_with_progress(
    request: TaskRequest,
    sender: &mpsc::Sender<TaskEventEnvelope>,
    request_id: RequestId,
    target_tool_id: &'static str,
    cancel: &AtomicBool,
) -> TaskResult {
    match request {
        TaskRequest::Ping {
            host,
            count,
            timeout_ms,
            payload_size,
            family,
        } => TaskResult::Ping(windows::ping_host_with_progress(
            &host,
            count,
            timeout_ms,
            payload_size,
            family,
            |sample, completed, total| {
                let message = sample.elapsed_ms.map_or_else(
                    || {
                        format!(
                            "{}：{}",
                            sample.address,
                            sample.error.as_deref().unwrap_or("失败")
                        )
                    },
                    |elapsed| format!("{}：{elapsed:.1} ms", sample.address),
                );
                let _ = sender.send(TaskEventEnvelope {
                    request_id,
                    target_tool_id,
                    event: TaskEvent::Progress {
                        message,
                        completed: Some(u64::from(completed)),
                        total: Some(u64::from(total)),
                    },
                });
            },
        )),
        TaskRequest::Mtr { host, config } => TaskResult::Mtr(windows::mtr_host_with_progress(
            &host,
            &config,
            cancel,
            |progress| {
                let _ = sender.send(TaskEventEnvelope {
                    request_id,
                    target_tool_id,
                    event: TaskEvent::MtrProgress(progress),
                });
            },
        )),
        request => execute(request),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{atomic::AtomicU64, mpsc};

    use super::{
        REQUEST_QUEUE_CAPACITY, TaskEvent, TaskRequest, TaskResult, WORKER_THREAD_COUNT,
        ordinary_task_events, try_enqueue,
    };

    #[test]
    fn worker_pool_limits_are_stable() {
        assert_eq!(WORKER_THREAD_COUNT, 4);
        assert_eq!(REQUEST_QUEUE_CAPACITY, 32);
    }

    #[test]
    fn requests_map_to_their_tool_boundary() {
        assert_eq!(
            TaskRequest::FileLocks { path: "a".into() }.target_tool_id(),
            "file-lock"
        );
        assert_eq!(TaskRequest::Ports.target_tool_id(), "port-inspector");
        assert_eq!(
            TaskRequest::TerminateProcess { pid: 1 }.target_tool_id(),
            "process-inspector"
        );
    }

    #[test]
    fn full_queue_is_rejected_without_waiting_for_a_receiver() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let next_id = AtomicU64::new(1);
        assert_eq!(
            try_enqueue(&sender, &next_id, TaskRequest::Ports).unwrap(),
            1
        );
        let error = try_enqueue(&sender, &next_id, TaskRequest::Ports).unwrap_err();
        assert!(matches!(error, crate::model::AppError::QueueFull(_)));
    }

    #[test]
    fn ordinary_query_emits_only_one_finished_event() {
        let events = ordinary_task_events(7, "port-inspector", TaskResult::Ports(Ok(Vec::new())));
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].event, TaskEvent::Finished(_)));
    }
}
