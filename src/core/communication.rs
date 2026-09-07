//! TCP、UDP 与串口持久会话调度器。
//! 应用外壳只负责启动、停止和路由有界消息；所有持续收发、资源关闭与批量记录均在线程内完成。

use std::{
    collections::{HashMap, VecDeque},
    io::{self, Read, Write},
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, ToSocketAddrs, UdpSocket},
    sync::mpsc,
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::{
    model::{
        AppError, COMMUNICATION_QUEUE_CAPACITY, CommunicationCommand, CommunicationConfig,
        CommunicationDirection, CommunicationEvent, CommunicationEventEnvelope,
        CommunicationIpFamily, CommunicationKind, CommunicationPeer, CommunicationRecord,
        CommunicationSendTarget, CommunicationSessionState, PayloadFormat, MAX_RECORD_BATCH,
        MAX_TCP_PENDING_BYTES, MAX_UDP_PAYLOAD_BYTES, RECORD_BATCH_INTERVAL_MS, SerialDataBits,
        SerialDebugConfig, SerialFlowControl, SerialParity, SerialStopBits, TcpDebugConfig,
        TcpDebugMode, UdpDebugConfig,
    },
    platform::windows::local_time_hms_millis,
};

const IO_POLL_INTERVAL: Duration = Duration::from_millis(5);
const PEER_UPDATE_INTERVAL: Duration = Duration::from_millis(100);
const TCP_READ_BUFFER_BYTES: usize = 64 * 1024;
const SERIAL_READ_BUFFER_BYTES: usize = 16 * 1024;
const MAX_TCP_SERVER_CLIENTS: usize = 32;

struct SessionHandle {
    sender: mpsc::SyncSender<CommunicationCommand>,
    receiver: mpsc::Receiver<CommunicationEventEnvelope>,
    join: Option<JoinHandle<()>>,
    summary: String,
    state: CommunicationSessionState,
    status_detail: String,
}

/// 首页使用的活动通信会话快照，仅保留当前进程内存中的连接摘要与状态。
#[derive(Clone, Debug)]
pub struct ActiveCommunicationSession {
    pub kind: CommunicationKind,
    pub summary: String,
    pub state: CommunicationSessionState,
    pub status_detail: String,
}

/// 每个工具至多拥有一个当前会话；替换会话时旧 generation 的事件会在应用外壳路由前被丢弃。
pub struct CommunicationDispatcher {
    sessions: HashMap<CommunicationKind, SessionHandle>,
    generations: HashMap<CommunicationKind, u64>,
    retired: Vec<JoinHandle<()>>,
    next_generation: u64,
}

impl Default for CommunicationDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

impl CommunicationDispatcher {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            generations: HashMap::new(),
            retired: Vec::new(),
            next_generation: 1,
        }
    }

    pub fn start(&mut self, config: CommunicationConfig) -> Result<u64, AppError> {
        config.validate().map_err(AppError::InvalidInput)?;
        let kind = config.kind();
        let summary = config.session_summary();
        self.stop_current(kind);
        self.reap_retired();

        let generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1);
        let (command_sender, command_receiver) = mpsc::sync_channel(COMMUNICATION_QUEUE_CAPACITY);
        let (event_sender, event_receiver) = mpsc::sync_channel(COMMUNICATION_QUEUE_CAPACITY);
        let join = thread::Builder::new()
            .name(format!("toolbox-{}-{generation}", kind.tool_id()))
            .spawn(move || run_session(config, generation, command_receiver, event_sender))
            .map_err(|error| {
                AppError::WorkerBusy(format!("无法创建{}会话线程：{error}", kind.tool_id()))
            })?;

        self.generations.insert(kind, generation);
        self.sessions.insert(
            kind,
            SessionHandle {
                sender: command_sender,
                receiver: event_receiver,
                join: Some(join),
                summary,
                state: CommunicationSessionState::Starting,
                status_detail: "正在启动会话".into(),
            },
        );
        Ok(generation)
    }

    pub fn send(
        &mut self,
        kind: CommunicationKind,
        command: CommunicationCommand,
    ) -> Result<(), AppError> {
        self.reap_retired();
        let Some(session) = self.sessions.get(&kind) else {
            return Err(AppError::InvalidInput("请先启动通信会话。".into()));
        };
        session
            .sender
            .try_send(command)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    AppError::QueueFull("通信命令队列已满，请稍后重试。".into())
                }
                mpsc::TrySendError::Disconnected(_) => {
                    AppError::WorkerBusy("通信会话已停止，请重新连接。".into())
                }
            })
    }

    pub fn stop(&mut self, kind: CommunicationKind) -> Result<(), AppError> {
        let Some(session) = self.sessions.get(&kind) else {
            return Ok(());
        };
        session
            .sender
            .try_send(CommunicationCommand::Stop)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    AppError::QueueFull("通信命令队列已满，暂时无法提交停止请求。".into())
                }
                mpsc::TrySendError::Disconnected(_) => {
                    AppError::WorkerBusy("通信会话已经停止。".into())
                }
            })
    }

    #[cfg(test)]
    pub fn accepts(&self, envelope: &CommunicationEventEnvelope) -> bool {
        self.generations.get(&envelope.kind).copied() == Some(envelope.generation)
    }

    pub fn drain_events(&mut self) -> Vec<CommunicationEventEnvelope> {
        self.reap_retired();
        let mut events = Vec::new();
        let generations = &self.generations;
        for session in self.sessions.values_mut() {
            while let Ok(event) = session.receiver.try_recv() {
                if generations.get(&event.kind).copied() == Some(event.generation) {
                    if let CommunicationEvent::Status { state, detail } = &event.event {
                        session.state = *state;
                        session.status_detail = detail.clone();
                    }
                    events.push(event);
                }
            }
        }
        events
    }

    /// 返回仍在运行的通信会话及其最新状态快照。
    pub fn active_sessions(&self) -> Vec<ActiveCommunicationSession> {
        let mut sessions = self
            .sessions
            .iter()
            .filter(|(_, session)| {
                session
                    .join
                    .as_ref()
                    .is_some_and(|join| !join.is_finished())
                    && !matches!(
                        session.state,
                        CommunicationSessionState::Stopped | CommunicationSessionState::Failed
                    )
            })
            .map(|(kind, session)| ActiveCommunicationSession {
                kind: *kind,
                summary: session.summary.clone(),
                state: session.state,
                status_detail: session.status_detail.clone(),
            })
            .collect::<Vec<_>>();
        sessions.sort_by_key(|session| session.kind.tool_id());
        sessions
    }

    fn stop_current(&mut self, kind: CommunicationKind) {
        if let Some(mut session) = self.sessions.remove(&kind) {
            let _ = session.sender.try_send(CommunicationCommand::Stop);
            if let Some(join) = session.join.take() {
                self.retired.push(join);
            }
        }
    }

    fn reap_retired(&mut self) {
        let mut index = 0;
        while index < self.retired.len() {
            if self.retired[index].is_finished() {
                let join = self.retired.swap_remove(index);
                let _ = join.join();
            } else {
                index += 1;
            }
        }
    }
}

impl Drop for CommunicationDispatcher {
    fn drop(&mut self) {
        let sessions = std::mem::take(&mut self.sessions);
        for session in sessions.into_values() {
            let SessionHandle {
                sender,
                receiver,
                join,
                ..
            } = session;
            let _ = sender.try_send(CommunicationCommand::Stop);
            drop(sender);
            drop(receiver);
            if let Some(join) = join {
                let _ = join.join();
            }
        }
        for join in self.retired.drain(..) {
            let _ = join.join();
        }
    }
}

fn run_session(
    config: CommunicationConfig,
    generation: u64,
    command_receiver: mpsc::Receiver<CommunicationCommand>,
    event_sender: mpsc::SyncSender<CommunicationEventEnvelope>,
) {
    let kind = config.kind();
    let mut emitter = EventEmitter::new(kind, generation, event_sender);
    emitter.status(CommunicationSessionState::Starting, "正在启动会话");
    match config {
        CommunicationConfig::Tcp(config) => run_tcp(config, &command_receiver, &mut emitter),
        CommunicationConfig::Udp(config) => run_udp(config, &command_receiver, &mut emitter),
        CommunicationConfig::Serial(config) => run_serial(config, &command_receiver, &mut emitter),
    }
    emitter.finish();
}

struct EventEmitter {
    kind: CommunicationKind,
    generation: u64,
    sender: mpsc::SyncSender<CommunicationEventEnvelope>,
    records: Vec<CommunicationRecord>,
    last_flush: Instant,
    dropped_records: u64,
}

impl EventEmitter {
    fn new(
        kind: CommunicationKind,
        generation: u64,
        sender: mpsc::SyncSender<CommunicationEventEnvelope>,
    ) -> Self {
        Self {
            kind,
            generation,
            sender,
            records: Vec::with_capacity(MAX_RECORD_BATCH),
            last_flush: Instant::now(),
            dropped_records: 0,
        }
    }

    fn status(&mut self, state: CommunicationSessionState, detail: impl Into<String>) {
        self.flush(true);
        let _ = self.sender.send(CommunicationEventEnvelope {
            kind: self.kind,
            generation: self.generation,
            event: CommunicationEvent::Status {
                state,
                detail: detail.into(),
            },
        });
    }

    fn peers(&mut self, peers: Vec<CommunicationPeer>) {
        self.flush(true);
        let _ = self.try_event(CommunicationEvent::Peers(peers));
    }

    fn record(
        &mut self,
        direction: CommunicationDirection,
        endpoint: impl Into<String>,
        payload: Vec<u8>,
        payload_format: Option<PayloadFormat>,
        note: Option<String>,
    ) {
        self.records.push(CommunicationRecord {
            timestamp: local_time_hms_millis(),
            direction,
            endpoint: endpoint.into(),
            payload,
            payload_format,
            note,
        });
        self.flush(false);
    }

    fn flush(&mut self, force: bool) {
        let elapsed = self.last_flush.elapsed();
        if !force
            && self.records.len() < MAX_RECORD_BATCH
            && elapsed < Duration::from_millis(RECORD_BATCH_INTERVAL_MS)
        {
            return;
        }
        if !self.records.is_empty() {
            let records = std::mem::take(&mut self.records);
            let count = records.len() as u64;
            if !self.try_event(CommunicationEvent::Records(records)) {
                self.dropped_records = self.dropped_records.saturating_add(count);
            }
        }
        if self.dropped_records > 0 {
            let dropped = self.dropped_records;
            if self.try_event(CommunicationEvent::QueueDropped(dropped)) {
                self.dropped_records = 0;
            }
        }
        self.last_flush = Instant::now();
    }

    fn finish(&mut self) {
        self.flush(true);
        if self.dropped_records == 0 {
            return;
        }
        let dropped = self.dropped_records;
        if self
            .sender
            .send(CommunicationEventEnvelope {
                kind: self.kind,
                generation: self.generation,
                event: CommunicationEvent::QueueDropped(dropped),
            })
            .is_ok()
        {
            self.dropped_records = 0;
        }
    }

    fn try_event(&self, event: CommunicationEvent) -> bool {
        self.sender
            .try_send(CommunicationEventEnvelope {
                kind: self.kind,
                generation: self.generation,
                event,
            })
            .is_ok()
    }
}

fn run_tcp(
    config: TcpDebugConfig,
    command_receiver: &mpsc::Receiver<CommunicationCommand>,
    emitter: &mut EventEmitter,
) {
    match config.mode {
        TcpDebugMode::Client => run_tcp_client(config, command_receiver, emitter),
        TcpDebugMode::Server => run_tcp_server(config, command_receiver, emitter),
    }
}

fn run_tcp_client(
    config: TcpDebugConfig,
    command_receiver: &mpsc::Receiver<CommunicationCommand>,
    emitter: &mut EventEmitter,
) {
    let addresses = match resolve_addresses(&config.address, config.port, config.family) {
        Ok(addresses) => addresses,
        Err(error) => {
            emitter.status(
                CommunicationSessionState::Failed,
                format!("地址解析失败：{error}"),
            );
            return;
        }
    };
    let timeout = Duration::from_millis(u64::from(config.connect_timeout_ms));
    let deadline = Instant::now() + timeout;
    let mut last_error = None;
    let mut stream = None;
    for address in addresses {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match TcpStream::connect_timeout(&address, remaining) {
            Ok(candidate) => {
                stream = Some(candidate);
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    let Some(stream) = stream else {
        emitter.status(
            CommunicationSessionState::Failed,
            format!(
                "TCP 连接失败：{}",
                last_error.map_or_else(|| "没有可用地址".into(), |error| error.to_string())
            ),
        );
        return;
    };
    if let Err(error) = stream.set_nonblocking(true) {
        emitter.status(
            CommunicationSessionState::Failed,
            format!("无法启用非阻塞收发：{error}"),
        );
        return;
    }
    let peer = stream
        .peer_addr()
        .map_or_else(|_| config.address.clone(), |address| address.to_string());
    emitter.status(
        CommunicationSessionState::Connected,
        format!("已连接 {peer}"),
    );
    let mut connection = TcpConnection::new(1, stream, peer);
    let mut read_buffer = vec![0_u8; TCP_READ_BUFFER_BYTES];
    let mut running = true;
    while running {
        running = handle_tcp_client_commands(command_receiver, &mut connection, emitter);
        if !running {
            break;
        }
        if let Err(error) = connection.flush_pending() {
            emitter.status(
                CommunicationSessionState::Failed,
                format!("TCP 写入失败：{error}"),
            );
            break;
        }
        match connection.stream.read(&mut read_buffer) {
            Ok(0) => {
                emitter.status(CommunicationSessionState::Stopped, "远端已关闭 TCP 连接");
                return;
            }
            Ok(count) => emitter.record(
                CommunicationDirection::Incoming,
                &connection.endpoint,
                read_buffer[..count].to_vec(),
                None,
                None,
            ),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => {
                emitter.status(
                    CommunicationSessionState::Failed,
                    format!("TCP 读取失败：{error}"),
                );
                return;
            }
        }
        emitter.flush(false);
        thread::sleep(IO_POLL_INTERVAL);
    }
    emitter.status(CommunicationSessionState::Stopped, "TCP 会话已停止");
}

fn handle_tcp_client_commands(
    receiver: &mpsc::Receiver<CommunicationCommand>,
    connection: &mut TcpConnection,
    emitter: &mut EventEmitter,
) -> bool {
    loop {
        match receiver.try_recv() {
            Ok(CommunicationCommand::Stop) => return false,
            Ok(CommunicationCommand::Send {
                payload, format, ..
            }) => {
                if payload.is_empty() {
                    continue;
                }
                if let Err(error) = connection.enqueue(&payload) {
                    emitter.status(CommunicationSessionState::Failed, error);
                    return false;
                }
                emitter.record(
                    CommunicationDirection::Outgoing,
                    &connection.endpoint,
                    payload,
                    Some(format),
                    None,
                );
            }
            Ok(CommunicationCommand::SetRts(_) | CommunicationCommand::SetDtr(_)) => {}
            Err(mpsc::TryRecvError::Empty) => return true,
            Err(mpsc::TryRecvError::Disconnected) => return false,
        }
    }
}

fn run_tcp_server(
    config: TcpDebugConfig,
    command_receiver: &mpsc::Receiver<CommunicationCommand>,
    emitter: &mut EventEmitter,
) {
    let bind_address = match resolve_first(&config.address, config.port, config.family) {
        Ok(address) => address,
        Err(error) => {
            emitter.status(
                CommunicationSessionState::Failed,
                format!("监听地址无效：{error}"),
            );
            return;
        }
    };
    let listener = match TcpListener::bind(bind_address) {
        Ok(listener) => listener,
        Err(error) => {
            emitter.status(
                CommunicationSessionState::Failed,
                format!("TCP 监听失败：{error}"),
            );
            return;
        }
    };
    if let Err(error) = listener.set_nonblocking(true) {
        emitter.status(
            CommunicationSessionState::Failed,
            format!("无法启用非阻塞监听：{error}"),
        );
        return;
    }
    let local = listener
        .local_addr()
        .map_or_else(|_| bind_address.to_string(), |address| address.to_string());
    emitter.status(
        CommunicationSessionState::Listening,
        format!("正在监听 {local}"),
    );

    let mut clients = Vec::<TcpConnection>::new();
    let mut next_peer_id = 1_u64;
    let mut read_buffer = vec![0_u8; TCP_READ_BUFFER_BYTES];
    let mut peer_stats_dirty = false;
    let mut last_peer_update = Instant::now();
    let mut running = true;
    while running {
        loop {
            match listener.accept() {
                Ok((stream, address)) => {
                    if clients.len() >= MAX_TCP_SERVER_CLIENTS {
                        emitter.record(
                            CommunicationDirection::Status,
                            address.to_string(),
                            Vec::new(),
                            None,
                            Some("已达到 32 个客户端上限，拒绝新连接".into()),
                        );
                        continue;
                    }
                    if let Err(error) = stream.set_nonblocking(true) {
                        emitter.record(
                            CommunicationDirection::Status,
                            address.to_string(),
                            Vec::new(),
                            None,
                            Some(format!("客户端初始化失败：{error}")),
                        );
                        continue;
                    }
                    clients.push(TcpConnection::new(
                        next_peer_id,
                        stream,
                        address.to_string(),
                    ));
                    next_peer_id = next_peer_id.saturating_add(1);
                    emit_peers(&clients, emitter);
                    last_peer_update = Instant::now();
                    peer_stats_dirty = false;
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => {
                    emitter.status(
                        CommunicationSessionState::Failed,
                        format!("TCP 接受连接失败：{error}"),
                    );
                    return;
                }
            }
        }

        loop {
            match command_receiver.try_recv() {
                Ok(CommunicationCommand::Stop) => {
                    running = false;
                    break;
                }
                Ok(CommunicationCommand::Send {
                    payload,
                    target,
                    format,
                }) => {
                    if payload.is_empty() {
                        continue;
                    }
                    if !queue_server_payload(&mut clients, &payload, target, format, emitter) {
                        running = false;
                        break;
                    }
                }
                Ok(CommunicationCommand::SetRts(_) | CommunicationCommand::SetDtr(_)) => {}
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    running = false;
                    break;
                }
            }
        }

        let mut disconnected = Vec::new();
        for (index, client) in clients.iter_mut().enumerate() {
            match client.flush_pending() {
                Ok(written) => {
                    peer_stats_dirty |= written > 0;
                }
                Err(error) => {
                    emitter.record(
                        CommunicationDirection::Status,
                        &client.endpoint,
                        Vec::new(),
                        None,
                        Some(format!("TCP 写入失败：{error}")),
                    );
                    disconnected.push(index);
                    continue;
                }
            }
            match client.stream.read(&mut read_buffer) {
                Ok(0) => disconnected.push(index),
                Ok(count) => {
                    client.received_bytes = client.received_bytes.saturating_add(count as u64);
                    peer_stats_dirty = true;
                    emitter.record(
                        CommunicationDirection::Incoming,
                        &client.endpoint,
                        read_buffer[..count].to_vec(),
                        None,
                        None,
                    );
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => {
                    emitter.record(
                        CommunicationDirection::Status,
                        &client.endpoint,
                        Vec::new(),
                        None,
                        Some(format!("TCP 读取失败：{error}")),
                    );
                    disconnected.push(index);
                }
            }
        }
        if !disconnected.is_empty() {
            disconnected.sort_unstable();
            disconnected.dedup();
            for index in disconnected.into_iter().rev() {
                clients.remove(index);
            }
            emit_peers(&clients, emitter);
            last_peer_update = Instant::now();
            peer_stats_dirty = false;
        } else if peer_stats_dirty && last_peer_update.elapsed() >= PEER_UPDATE_INTERVAL {
            emit_peers(&clients, emitter);
            last_peer_update = Instant::now();
            peer_stats_dirty = false;
        }
        emitter.flush(false);
        thread::sleep(IO_POLL_INTERVAL);
    }
    emitter.status(CommunicationSessionState::Stopped, "TCP 监听已停止");
}

fn queue_server_payload(
    clients: &mut [TcpConnection],
    payload: &[u8],
    target: CommunicationSendTarget,
    format: PayloadFormat,
    emitter: &mut EventEmitter,
) -> bool {
    let mut matched = false;
    for client in clients {
        let selected = tcp_target_selects(&target, client.id);
        if !selected {
            continue;
        }
        matched = true;
        if let Err(error) = client.enqueue(payload) {
            emitter.status(CommunicationSessionState::Failed, error);
            return false;
        }
        emitter.record(
            CommunicationDirection::Outgoing,
            &client.endpoint,
            payload.to_vec(),
            Some(format),
            None,
        );
    }
    if !matched {
        emitter.record(
            CommunicationDirection::Status,
            "TCP",
            Vec::new(),
            None,
            Some("没有可用的发送目标".into()),
        );
    }
    true
}

fn tcp_target_selects(target: &CommunicationSendTarget, client_id: u64) -> bool {
    match target {
        CommunicationSendTarget::TcpClient(id) => client_id == *id,
        CommunicationSendTarget::AllTcpClients | CommunicationSendTarget::Default => true,
        CommunicationSendTarget::UdpSource(_) => false,
    }
}

struct TcpConnection {
    id: u64,
    stream: TcpStream,
    endpoint: String,
    connected_at: String,
    received_bytes: u64,
    sent_bytes: u64,
    pending: VecDeque<u8>,
}

impl TcpConnection {
    fn new(id: u64, stream: TcpStream, endpoint: String) -> Self {
        Self {
            id,
            stream,
            endpoint,
            connected_at: local_time_hms_millis(),
            received_bytes: 0,
            sent_bytes: 0,
            pending: VecDeque::new(),
        }
    }

    fn enqueue(&mut self, payload: &[u8]) -> Result<(), String> {
        if self.pending.len().saturating_add(payload.len()) > MAX_TCP_PENDING_BYTES {
            return Err(format!(
                "{} 的待写队列已达到 1 MiB 上限，会话已停止。",
                self.endpoint
            ));
        }
        self.pending.extend(payload);
        Ok(())
    }

    fn flush_pending(&mut self) -> io::Result<usize> {
        let mut written_total = 0_usize;
        while !self.pending.is_empty() {
            let contiguous = self.pending.make_contiguous();
            match self.stream.write(contiguous) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "写入返回 0 字节")),
                Ok(count) => {
                    self.pending.drain(..count);
                    written_total = written_total.saturating_add(count);
                    self.sent_bytes = self.sent_bytes.saturating_add(count as u64);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    return Ok(written_total);
                }
                Err(error) => return Err(error),
            }
        }
        Ok(written_total)
    }
}

fn emit_peers(clients: &[TcpConnection], emitter: &mut EventEmitter) {
    emitter.peers(
        clients
            .iter()
            .map(|client| CommunicationPeer {
                id: client.id,
                endpoint: client.endpoint.clone(),
                connected_at: client.connected_at.clone(),
                received_bytes: client.received_bytes,
                sent_bytes: client.sent_bytes,
            })
            .collect(),
    );
}

fn run_udp(
    config: UdpDebugConfig,
    command_receiver: &mpsc::Receiver<CommunicationCommand>,
    emitter: &mut EventEmitter,
) {
    let bind_address = match resolve_first(&config.local_address, config.local_port, config.family)
    {
        Ok(address) => address,
        Err(error) => {
            emitter.status(
                CommunicationSessionState::Failed,
                format!("绑定地址无效：{error}"),
            );
            return;
        }
    };
    let socket = match UdpSocket::bind(bind_address) {
        Ok(socket) => socket,
        Err(error) => {
            emitter.status(
                CommunicationSessionState::Failed,
                format!("UDP 绑定失败：{error}"),
            );
            return;
        }
    };
    if let Err(error) = socket.set_nonblocking(true) {
        emitter.status(
            CommunicationSessionState::Failed,
            format!("无法启用 UDP 非阻塞收发：{error}"),
        );
        return;
    }
    if let Err(error) = socket.set_broadcast(config.broadcast) {
        emitter.status(
            CommunicationSessionState::Failed,
            format!("广播配置失败：{error}"),
        );
        return;
    }

    let multicast_membership = match configure_multicast(&socket, &config) {
        Ok(value) => value,
        Err(error) => {
            emitter.status(CommunicationSessionState::Failed, error);
            return;
        }
    };
    let default_remote = if config.remote_address.trim().is_empty() {
        None
    } else {
        match resolve_first(&config.remote_address, config.remote_port, config.family) {
            Ok(address) => Some(address),
            Err(error) => {
                emitter.status(
                    CommunicationSessionState::Failed,
                    format!("默认远端地址无效：{error}"),
                );
                return;
            }
        }
    };
    let local = socket
        .local_addr()
        .map_or_else(|_| bind_address.to_string(), |address| address.to_string());
    emitter.status(
        CommunicationSessionState::Connected,
        format!("UDP 已绑定 {local}"),
    );

    let mut buffer = vec![0_u8; MAX_UDP_PAYLOAD_BYTES];
    let mut running = true;
    while running {
        loop {
            match command_receiver.try_recv() {
                Ok(CommunicationCommand::Stop) => {
                    running = false;
                    break;
                }
                Ok(CommunicationCommand::Send {
                    payload,
                    target,
                    format,
                }) => {
                    if payload.len() > MAX_UDP_PAYLOAD_BYTES {
                        emitter.record(
                            CommunicationDirection::Status,
                            "UDP",
                            Vec::new(),
                            None,
                            Some("UDP 单次载荷不能超过 65507 字节".into()),
                        );
                        continue;
                    }
                    let destination = match target {
                        CommunicationSendTarget::UdpSource(address) => Some(address),
                        _ => default_remote,
                    };
                    let Some(destination) = destination else {
                        emitter.record(
                            CommunicationDirection::Status,
                            "UDP",
                            Vec::new(),
                            None,
                            Some("未配置默认远端，也未选择接收来源".into()),
                        );
                        continue;
                    };
                    match socket.send_to(&payload, destination) {
                        Ok(count) if count == payload.len() => emitter.record(
                            CommunicationDirection::Outgoing,
                            destination.to_string(),
                            payload,
                            Some(format),
                            None,
                        ),
                        Ok(count) => emitter.record(
                            CommunicationDirection::Status,
                            destination.to_string(),
                            Vec::new(),
                            None,
                            Some(format!("UDP 仅发送 {count} 字节")),
                        ),
                        Err(error) => emitter.record(
                            CommunicationDirection::Status,
                            destination.to_string(),
                            Vec::new(),
                            None,
                            Some(format!("UDP 发送失败：{error}")),
                        ),
                    }
                }
                Ok(CommunicationCommand::SetRts(_) | CommunicationCommand::SetDtr(_)) => {}
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    running = false;
                    break;
                }
            }
        }
        loop {
            match socket.recv_from(&mut buffer) {
                Ok((count, source)) => emitter.record(
                    CommunicationDirection::Incoming,
                    source.to_string(),
                    buffer[..count].to_vec(),
                    None,
                    None,
                ),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => {
                    emitter.status(
                        CommunicationSessionState::Failed,
                        format!("UDP 接收失败：{error}"),
                    );
                    return;
                }
            }
        }
        emitter.flush(false);
        thread::sleep(IO_POLL_INTERVAL);
    }
    if let Some((group, interface)) = multicast_membership {
        let _ = socket.leave_multicast_v4(&group, &interface);
    }
    emitter.status(CommunicationSessionState::Stopped, "UDP 会话已停止");
}

fn configure_multicast(
    socket: &UdpSocket,
    config: &UdpDebugConfig,
) -> Result<Option<(Ipv4Addr, Ipv4Addr)>, String> {
    let Some(multicast) = &config.multicast else {
        return Ok(None);
    };
    let group = multicast
        .group
        .parse::<Ipv4Addr>()
        .map_err(|error| format!("组播地址无效：{error}"))?;
    let interface = multicast
        .interface
        .parse::<Ipv4Addr>()
        .map_err(|error| format!("组播接口无效：{error}"))?;
    socket
        .set_multicast_ttl_v4(multicast.ttl)
        .map_err(|error| format!("组播 TTL 配置失败：{error}"))?;
    socket
        .set_multicast_loop_v4(multicast.loopback)
        .map_err(|error| format!("组播回环配置失败：{error}"))?;
    socket
        .join_multicast_v4(&group, &interface)
        .map_err(|error| format!("加入组播组失败：{error}"))?;
    Ok(Some((group, interface)))
}

fn run_serial(
    config: SerialDebugConfig,
    command_receiver: &mpsc::Receiver<CommunicationCommand>,
    emitter: &mut EventEmitter,
) {
    let mut port = match serialport::new(&config.port_name, config.baud_rate)
        .data_bits(serial_data_bits(config.data_bits))
        .parity(serial_parity(config.parity))
        .stop_bits(serial_stop_bits(config.stop_bits))
        .flow_control(serial_flow_control(config.flow_control))
        .timeout(Duration::from_millis(config.read_timeout_ms))
        .open()
    {
        Ok(port) => port,
        Err(error) => {
            emitter.status(
                CommunicationSessionState::Failed,
                format!("串口打开失败：{error}"),
            );
            return;
        }
    };
    emitter.status(
        CommunicationSessionState::Connected,
        format!("已独占打开 {}", config.port_name),
    );
    let mut buffer = vec![0_u8; SERIAL_READ_BUFFER_BYTES];
    let mut running = true;
    while running {
        loop {
            match command_receiver.try_recv() {
                Ok(CommunicationCommand::Stop) => {
                    running = false;
                    break;
                }
                Ok(CommunicationCommand::Send {
                    payload, format, ..
                }) => {
                    if payload.is_empty() {
                        continue;
                    }
                    if let Err(error) = port.write_all(&payload) {
                        emitter.status(
                            CommunicationSessionState::Failed,
                            format!("串口写入失败或设备已断开：{error}"),
                        );
                        return;
                    }
                    emitter.record(
                        CommunicationDirection::Outgoing,
                        &config.port_name,
                        payload,
                        Some(format),
                        None,
                    );
                }
                Ok(CommunicationCommand::SetRts(enabled)) => {
                    if let Err(error) = port.write_request_to_send(enabled) {
                        emitter.status(
                            CommunicationSessionState::Failed,
                            format!("设置 RTS 失败或设备已断开：{error}"),
                        );
                        return;
                    }
                    emitter.record(
                        CommunicationDirection::Status,
                        &config.port_name,
                        Vec::new(),
                        None,
                        Some(format!("RTS 已{}", if enabled { "置位" } else { "复位" })),
                    );
                }
                Ok(CommunicationCommand::SetDtr(enabled)) => {
                    if let Err(error) = port.write_data_terminal_ready(enabled) {
                        emitter.status(
                            CommunicationSessionState::Failed,
                            format!("设置 DTR 失败或设备已断开：{error}"),
                        );
                        return;
                    }
                    emitter.record(
                        CommunicationDirection::Status,
                        &config.port_name,
                        Vec::new(),
                        None,
                        Some(format!("DTR 已{}", if enabled { "置位" } else { "复位" })),
                    );
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    running = false;
                    break;
                }
            }
        }
        if !running {
            break;
        }
        match port.read(&mut buffer) {
            Ok(0) => {}
            Ok(count) => emitter.record(
                CommunicationDirection::Incoming,
                &config.port_name,
                buffer[..count].to_vec(),
                None,
                None,
            ),
            Err(error) if serial_read_should_wait(error.kind()) => {}
            Err(error) => {
                emitter.status(
                    CommunicationSessionState::Failed,
                    format!("串口读取失败或设备已拔出：{error}"),
                );
                return;
            }
        }
        emitter.flush(false);
    }
    emitter.status(CommunicationSessionState::Stopped, "串口会话已关闭");
}

fn serial_read_should_wait(kind: io::ErrorKind) -> bool {
    matches!(kind, io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock)
}

fn serial_data_bits(value: SerialDataBits) -> serialport::DataBits {
    match value {
        SerialDataBits::Five => serialport::DataBits::Five,
        SerialDataBits::Six => serialport::DataBits::Six,
        SerialDataBits::Seven => serialport::DataBits::Seven,
        SerialDataBits::Eight => serialport::DataBits::Eight,
    }
}

fn serial_parity(value: SerialParity) -> serialport::Parity {
    match value {
        SerialParity::None => serialport::Parity::None,
        SerialParity::Odd => serialport::Parity::Odd,
        SerialParity::Even => serialport::Parity::Even,
    }
}

fn serial_stop_bits(value: SerialStopBits) -> serialport::StopBits {
    match value {
        SerialStopBits::One => serialport::StopBits::One,
        SerialStopBits::Two => serialport::StopBits::Two,
    }
}

fn serial_flow_control(value: SerialFlowControl) -> serialport::FlowControl {
    match value {
        SerialFlowControl::None => serialport::FlowControl::None,
        SerialFlowControl::Software => serialport::FlowControl::Software,
        SerialFlowControl::Hardware => serialport::FlowControl::Hardware,
    }
}

fn resolve_first(host: &str, port: u16, family: CommunicationIpFamily) -> io::Result<SocketAddr> {
    resolve_addresses(host, port, family)?
        .into_iter()
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::AddrNotAvailable, "没有匹配地址族的地址"))
}

fn resolve_addresses(
    host: &str,
    port: u16,
    family: CommunicationIpFamily,
) -> io::Result<Vec<SocketAddr>> {
    let addresses = (host.trim(), port)
        .to_socket_addrs()?
        .filter(|address| {
            matches!(
                (family, address.ip()),
                (CommunicationIpFamily::V4, IpAddr::V4(_))
                    | (CommunicationIpFamily::V6, IpAddr::V6(_))
            )
        })
        .collect::<Vec<_>>();
    if addresses.is_empty() {
        Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "没有匹配地址族的地址",
        ))
    } else {
        Ok(addresses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_filter_only_accepts_the_current_session() {
        let mut dispatcher = CommunicationDispatcher::new();
        dispatcher.generations.insert(CommunicationKind::Tcp, 8);
        let current = CommunicationEventEnvelope {
            kind: CommunicationKind::Tcp,
            generation: 8,
            event: CommunicationEvent::QueueDropped(0),
        };
        let stale = CommunicationEventEnvelope {
            generation: 7,
            ..current.clone()
        };
        assert!(dispatcher.accepts(&current));
        assert!(!dispatcher.accepts(&stale));
    }

    #[test]
    fn full_event_queue_reports_dropped_record_batch_after_capacity_returns() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut emitter = EventEmitter::new(CommunicationKind::Udp, 3, sender);
        emitter.status(CommunicationSessionState::Connected, "ready");
        emitter.record(
            CommunicationDirection::Incoming,
            "loopback",
            vec![1, 2, 3],
            None,
            None,
        );
        emitter.flush(true);
        assert!(matches!(
            receiver.try_recv().unwrap().event,
            CommunicationEvent::Status { .. }
        ));
        emitter.flush(true);
        assert_eq!(
            receiver.try_recv().unwrap().event,
            CommunicationEvent::QueueDropped(1)
        );
    }

    #[test]
    fn tcp_pending_queue_rejects_more_than_one_mebibyte() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let join = thread::spawn(move || TcpStream::connect(address).unwrap());
        let (stream, _) = listener.accept().unwrap();
        let _client = join.join().unwrap();
        let mut connection = TcpConnection::new(1, stream, "loopback".into());
        assert!(connection.enqueue(&vec![0; MAX_TCP_PENDING_BYTES]).is_ok());
        assert!(connection.enqueue(&[1]).is_err());
    }

    #[test]
    fn tcp_loopback_transfers_raw_bytes_and_target_selection_is_exact() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let join = thread::spawn(move || {
            let mut stream = TcpStream::connect(address).unwrap();
            stream.write_all(b"tcp-loopback").unwrap();
        });
        let (mut stream, _) = listener.accept().unwrap();
        let mut received = [0_u8; 12];
        stream.read_exact(&mut received).unwrap();
        join.join().unwrap();
        assert_eq!(&received, b"tcp-loopback");
        assert!(tcp_target_selects(
            &CommunicationSendTarget::TcpClient(7),
            7
        ));
        assert!(!tcp_target_selects(
            &CommunicationSendTarget::TcpClient(7),
            8
        ));
        assert!(tcp_target_selects(
            &CommunicationSendTarget::AllTcpClients,
            8
        ));
    }

    #[test]
    fn udp_ipv4_and_ipv6_loopback_preserve_datagram_boundaries() {
        for (bind, destination) in [("127.0.0.1:0", "127.0.0.1:0"), ("[::1]:0", "[::1]:0")] {
            let receiver = UdpSocket::bind(bind).unwrap();
            receiver
                .set_read_timeout(Some(Duration::from_secs(1)))
                .unwrap();
            let receiver_address = receiver.local_addr().unwrap();
            let sender = UdpSocket::bind(destination).unwrap();
            sender.send_to(b"one-datagram", receiver_address).unwrap();
            let mut buffer = [0_u8; 64];
            let (count, _) = receiver.recv_from(&mut buffer).unwrap();
            assert_eq!(&buffer[..count], b"one-datagram");
        }
    }

    #[test]
    fn serial_configuration_maps_and_disconnect_errors_are_not_waits() {
        assert_eq!(
            serial_data_bits(SerialDataBits::Eight),
            serialport::DataBits::Eight
        );
        assert_eq!(serial_parity(SerialParity::Odd), serialport::Parity::Odd);
        assert_eq!(
            serial_stop_bits(SerialStopBits::Two),
            serialport::StopBits::Two
        );
        assert_eq!(
            serial_flow_control(SerialFlowControl::Hardware),
            serialport::FlowControl::Hardware
        );
        assert!(serial_read_should_wait(io::ErrorKind::TimedOut));
        assert!(!serial_read_should_wait(io::ErrorKind::BrokenPipe));
    }

    #[test]
    fn ipv4_and_ipv6_loopback_resolution_preserves_family() {
        assert!(
            resolve_first("127.0.0.1", 9, CommunicationIpFamily::V4)
                .unwrap()
                .is_ipv4()
        );
        assert!(
            resolve_first("::1", 9, CommunicationIpFamily::V6)
                .unwrap()
                .is_ipv6()
        );
    }
}
