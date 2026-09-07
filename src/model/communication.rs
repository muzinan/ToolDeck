//! 持久通信调试工具的领域模型。
//! 本模块定义跨 UI、应用外壳与会话线程传递的配置、命令、事件和有界日志，不直接打开网络或串口资源。

use std::{collections::VecDeque, net::SocketAddr};

/// 每个通信会话的命令与事件队列容量。
pub const COMMUNICATION_QUEUE_CAPACITY: usize = 64;
/// 单次事件批量携带的最大记录数。
pub const MAX_RECORD_BATCH: usize = 128;
/// 记录批量最长等待时间，单位为毫秒。
pub const RECORD_BATCH_INTERVAL_MS: u64 = 50;
/// 单个工具保留的最大记录条数。
pub const MAX_LOG_RECORDS: usize = 10_000;
/// 单个工具保留的最大原始载荷字节数。
pub const MAX_LOG_PAYLOAD_BYTES: usize = 16 * 1024 * 1024;
/// TCP 单连接允许排队的最大待写字节数。
pub const MAX_TCP_PENDING_BYTES: usize = 1024 * 1024;
/// UDP 单个数据报允许的最大载荷。
pub const MAX_UDP_PAYLOAD_BYTES: usize = 65_507;

/// 持久通信工具类型，同时作为会话生命周期的唯一键。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CommunicationKind {
    Tcp,
    Udp,
    Serial,
}

impl CommunicationKind {
    pub fn tool_id(self) -> &'static str {
        match self {
            Self::Tcp => "tcp-debug",
            Self::Udp => "udp-debug",
            Self::Serial => "serial-debug",
        }
    }

}

/// 网络会话限定的地址族。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CommunicationIpFamily {
    #[default]
    V4,
    V6,
}

impl CommunicationIpFamily {
    pub fn label(self) -> &'static str {
        match self {
            Self::V4 => "IPv4",
            Self::V6 => "IPv6",
        }
    }
}

/// TCP 调试会话的角色。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TcpDebugMode {
    #[default]
    Client,
    Server,
}

impl TcpDebugMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Client => "客户端",
            Self::Server => "服务端",
        }
    }
}

/// TCP 会话启动配置；客户端时地址表示远端，服务端时表示本地监听地址。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TcpDebugConfig {
    pub mode: TcpDebugMode,
    pub family: CommunicationIpFamily,
    pub address: String,
    pub port: u16,
    pub connect_timeout_ms: u32,
}

impl TcpDebugConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.address.trim().is_empty() {
            return Err("地址不能为空。".into());
        }
        if self.port == 0 {
            return Err("端口必须在 1–65535 之间。".into());
        }
        if !(100..=30_000).contains(&self.connect_timeout_ms) {
            return Err("TCP 连接超时必须在 100–30000 ms 之间。".into());
        }
        Ok(())
    }
}

/// IPv4 组播配置；IPv6 组播不属于当前版本边界。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UdpMulticastConfig {
    pub group: String,
    pub interface: String,
    pub ttl: u32,
    pub loopback: bool,
}

/// UDP 会话启动配置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UdpDebugConfig {
    pub family: CommunicationIpFamily,
    pub local_address: String,
    pub local_port: u16,
    pub remote_address: String,
    pub remote_port: u16,
    pub broadcast: bool,
    pub multicast: Option<UdpMulticastConfig>,
}

impl UdpDebugConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.local_address.trim().is_empty() {
            return Err("本地绑定地址不能为空。".into());
        }
        if self.broadcast && self.family == CommunicationIpFamily::V6 {
            return Err("当前版本不支持 IPv6 广播。".into());
        }
        if self.remote_address.trim().is_empty() != (self.remote_port == 0) {
            return Err("默认远端地址和端口必须同时填写或同时留空。".into());
        }
        if let Some(multicast) = &self.multicast {
            if self.family != CommunicationIpFamily::V4 {
                return Err("当前版本仅支持 IPv4 组播。".into());
            }
            let group = multicast
                .group
                .parse::<std::net::Ipv4Addr>()
                .map_err(|_| "组播地址必须是有效 IPv4 地址。".to_owned())?;
            if !group.is_multicast() {
                return Err("组播地址必须位于 IPv4 组播地址范围。".into());
            }
            multicast
                .interface
                .parse::<std::net::Ipv4Addr>()
                .map_err(|_| "组播接口必须是有效的本地 IPv4 地址。".to_owned())?;
            if !(1..=255).contains(&multicast.ttl) {
                return Err("组播 TTL 必须在 1–255 之间。".into());
            }
        }
        Ok(())
    }
}

/// 串口数据位配置。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SerialDataBits {
    Five,
    Six,
    Seven,
    #[default]
    Eight,
}

impl SerialDataBits {
    pub const ALL: [Self; 4] = [Self::Five, Self::Six, Self::Seven, Self::Eight];

    pub fn label(self) -> &'static str {
        match self {
            Self::Five => "5",
            Self::Six => "6",
            Self::Seven => "7",
            Self::Eight => "8",
        }
    }
}

/// 串口校验位配置。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SerialParity {
    #[default]
    None,
    Odd,
    Even,
}

impl SerialParity {
    pub const ALL: [Self; 3] = [Self::None, Self::Odd, Self::Even];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "无",
            Self::Odd => "奇校验",
            Self::Even => "偶校验",
        }
    }
}

/// 串口停止位配置。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SerialStopBits {
    #[default]
    One,
    Two,
}

impl SerialStopBits {
    pub const ALL: [Self; 2] = [Self::One, Self::Two];

    pub fn label(self) -> &'static str {
        match self {
            Self::One => "1",
            Self::Two => "2",
        }
    }
}

/// 串口流控配置。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SerialFlowControl {
    #[default]
    None,
    Software,
    Hardware,
}

impl SerialFlowControl {
    pub const ALL: [Self; 3] = [Self::None, Self::Software, Self::Hardware];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "无",
            Self::Software => "软件",
            Self::Hardware => "硬件",
        }
    }
}

/// 串口会话启动配置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerialDebugConfig {
    pub port_name: String,
    pub baud_rate: u32,
    pub data_bits: SerialDataBits,
    pub parity: SerialParity,
    pub stop_bits: SerialStopBits,
    pub flow_control: SerialFlowControl,
    pub read_timeout_ms: u64,
}

impl SerialDebugConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.port_name.trim().is_empty() {
            return Err("请选择串口。".into());
        }
        if self.baud_rate == 0 {
            return Err("波特率必须大于 0。".into());
        }
        if !(10..=1_000).contains(&self.read_timeout_ms) {
            return Err("串口读取超时必须在 10–1000 ms 之间。".into());
        }
        Ok(())
    }
}

/// 三类会话的统一启动配置。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommunicationConfig {
    Tcp(TcpDebugConfig),
    Udp(UdpDebugConfig),
    Serial(SerialDebugConfig),
}

impl CommunicationConfig {
    pub fn kind(&self) -> CommunicationKind {
        match self {
            Self::Tcp(_) => CommunicationKind::Tcp,
            Self::Udp(_) => CommunicationKind::Udp,
            Self::Serial(_) => CommunicationKind::Serial,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::Tcp(config) => config.validate(),
            Self::Udp(config) => config.validate(),
            Self::Serial(config) => config.validate(),
        }
    }

    /// 返回仅在当前进程内存中展示的会话摘要，供首页运行会话区快速定位会话。
    pub fn session_summary(&self) -> String {
        match self {
            Self::Tcp(config) => match config.mode {
                TcpDebugMode::Client => format!("{}:{}", config.address.trim(), config.port),
                TcpDebugMode::Server => {
                    format!("监听 {}:{}", config.address.trim(), config.port)
                }
            },
            Self::Udp(config) => {
                let local = format!("{}:{}", config.local_address.trim(), config.local_port);
                if let Some(multicast) = &config.multicast {
                    format!("{} · 组播 {}:{}", local, multicast.group.trim(), config.remote_port)
                } else if config.remote_address.trim().is_empty() {
                    local
                } else {
                    format!(
                        "{} → {}:{}",
                        local,
                        config.remote_address.trim(),
                        config.remote_port
                    )
                }
            }
            Self::Serial(config) => {
                let parity = match config.parity {
                    SerialParity::None => "N",
                    SerialParity::Odd => "O",
                    SerialParity::Even => "E",
                };
                format!(
                    "{}，{}，{}{}{}",
                    config.port_name.trim(),
                    config.baud_rate,
                    config.data_bits.label(),
                    parity,
                    config.stop_bits.label()
                )
            }
        }
    }
}

/// 发送目标；不同协议只消费与自身相关的变体。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommunicationSendTarget {
    Default,
    TcpClient(u64),
    AllTcpClients,
    UdpSource(SocketAddr),
}

/// UI 向活动会话发送的命令。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommunicationCommand {
    Send {
        payload: Vec<u8>,
        target: CommunicationSendTarget,
        format: PayloadFormat,
    },
    SetRts(bool),
    SetDtr(bool),
    Stop,
}

/// 通信记录方向。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommunicationDirection {
    Incoming,
    Outgoing,
    Status,
}

impl CommunicationDirection {
    pub fn label(self) -> &'static str {
        match self {
            Self::Incoming => "RX",
            Self::Outgoing => "TX",
            Self::Status => "STATE",
        }
    }
}

/// 一条通信记录；原始载荷始终保留，文本或 HEX 仅在界面渲染时转换。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommunicationRecord {
    pub timestamp: String,
    pub direction: CommunicationDirection,
    pub endpoint: String,
    pub payload: Vec<u8>,
    pub payload_format: Option<PayloadFormat>,
    pub note: Option<String>,
}

impl CommunicationRecord {
    pub fn payload_bytes(&self) -> usize {
        self.payload.len()
    }

    /// 根据记录方向确定展示格式；发送记录固定使用发送时格式，接收记录跟随界面选择。
    pub fn display_format(&self, receive_format: PayloadFormat) -> PayloadFormat {
        if self.direction == CommunicationDirection::Outgoing {
            self.payload_format.unwrap_or(receive_format)
        } else {
            receive_format
        }
    }

    /// 返回界面、搜索、复制和导出共同使用的完整显示内容。
    pub fn display_content(&self, receive_format: PayloadFormat) -> String {
        self.note.as_deref().map_or_else(
            || render_payload(&self.payload, self.display_format(receive_format)),
            escape_log_field,
        )
    }
}

/// TCP 服务端向 UI 公布的客户端描述。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommunicationPeer {
    pub id: u64,
    pub endpoint: String,
    pub connected_at: String,
    pub received_bytes: u64,
    pub sent_bytes: u64,
}

/// 会话连接状态。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommunicationSessionState {
    Starting,
    Listening,
    Connected,
    Stopped,
    Failed,
}

impl CommunicationSessionState {
    /// 返回首页与工具页复用的状态名称。
    pub const fn label(self) -> &'static str {
        match self {
            Self::Starting => "启动中",
            Self::Listening => "监听中",
            Self::Connected => "已连接",
            Self::Stopped => "未连接",
            Self::Failed => "失败",
        }
    }
}

/// 会话线程发回 UI 的事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommunicationEvent {
    Status {
        state: CommunicationSessionState,
        detail: String,
    },
    Records(Vec<CommunicationRecord>),
    Peers(Vec<CommunicationPeer>),
    QueueDropped(u64),
}

/// 事件信封携带 generation，应用外壳必须在路由前丢弃旧会话事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommunicationEventEnvelope {
    pub kind: CommunicationKind,
    pub generation: u64,
    pub event: CommunicationEvent,
}

/// 串口列表项；USB 字段为空时表示系统未提供对应描述。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SerialPortDescriptor {
    pub port_name: String,
    pub port_type: String,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial_number: Option<String>,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
}

impl SerialPortDescriptor {
    pub fn display_name(&self) -> String {
        self.product.as_deref().map_or_else(
            || self.port_name.clone(),
            |product| format!("{} · {product}", self.port_name),
        )
    }
}

/// 发送或接收的显示编码。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PayloadFormat {
    #[default]
    Text,
    Hex,
}

impl PayloadFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "文本",
            Self::Hex => "HEX",
        }
    }
}

/// 将发送框内容转换为原始字节；HEX 要求每个空白分隔项恰好表示一个字节。
pub fn decode_payload(
    input: &str,
    format: PayloadFormat,
    append_crlf: bool,
) -> Result<Vec<u8>, String> {
    let mut payload = match format {
        PayloadFormat::Text => input.as_bytes().to_vec(),
        PayloadFormat::Hex => {
            let trimmed = input.trim();
            if trimmed.is_empty() {
                Vec::new()
            } else {
                trimmed
                    .split_whitespace()
                    .map(|part| {
                        if part.len() != 2 || !part.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                            return Err(format!(
                                "HEX 字节“{part}”无效，请使用空白分隔的两位字节。"
                            ));
                        }
                        u8::from_str_radix(part, 16)
                            .map_err(|_| format!("HEX 字节“{part}”超出有效范围。"))
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
        }
    };
    if append_crlf {
        payload.extend_from_slice(b"\r\n");
    }
    Ok(payload)
}

/// 按选定格式渲染载荷；文本模式保留可打印 Unicode，仅转义控制字符并标记无效 UTF-8。
pub fn render_payload(payload: &[u8], format: PayloadFormat) -> String {
    match format {
        PayloadFormat::Hex => payload
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect::<Vec<_>>()
            .join(" "),
        PayloadFormat::Text => {
            let lossy = String::from_utf8_lossy(payload);
            let invalid = matches!(&lossy, std::borrow::Cow::Owned(_));
            let escaped = lossy.chars().fold(String::new(), |mut output, character| {
                match character {
                    '\r' => output.push_str("\\r"),
                    '\n' => output.push_str("\\n"),
                    '\t' => output.push_str("\\t"),
                    '\0' => output.push_str("\\0"),
                    value if value.is_control() => {
                        output.extend(value.escape_default());
                    }
                    value => output.push(value),
                }
                output
            });
            if invalid {
                format!("{escaped} [含无效 UTF-8，已替换显示]")
            } else {
                escaped
            }
        }
    }
}

/// 单个工具的双重上限日志容器。
#[derive(Clone, Debug, Default)]
pub struct CommunicationLog {
    records: VecDeque<CommunicationRecord>,
    payload_bytes: usize,
    evicted_records: u64,
    queue_dropped_records: u64,
}

impl CommunicationLog {
    pub fn records(&self) -> &VecDeque<CommunicationRecord> {
        &self.records
    }

    pub fn queue_dropped_records(&self) -> u64 {
        self.queue_dropped_records
    }

    pub fn add_queue_dropped(&mut self, count: u64) {
        self.queue_dropped_records = self.queue_dropped_records.saturating_add(count);
    }

    pub fn push_batch(&mut self, records: Vec<CommunicationRecord>) {
        for record in records {
            let incoming_bytes = record.payload_bytes();
            while !self.records.is_empty()
                && (self.records.len() >= MAX_LOG_RECORDS
                    || self.payload_bytes.saturating_add(incoming_bytes) > MAX_LOG_PAYLOAD_BYTES)
            {
                self.evict_oldest();
            }
            if incoming_bytes > MAX_LOG_PAYLOAD_BYTES {
                self.evicted_records = self.evicted_records.saturating_add(1);
                continue;
            }
            self.payload_bytes = self.payload_bytes.saturating_add(incoming_bytes);
            self.records.push_back(record);
        }
    }

    pub fn clear(&mut self) {
        self.records.clear();
        self.payload_bytes = 0;
        self.evicted_records = 0;
        self.queue_dropped_records = 0;
    }

    pub fn export(&self, format: PayloadFormat) -> String {
        let mut output = String::from("时间\t方向\t端点\t字节数\t内容\n");
        for record in &self.records {
            let content = record.display_content(format);
            output.push_str(&format!(
                "{}\t{}\t{}\t{}\t{}\n",
                escape_log_field(&record.timestamp),
                record.direction.label(),
                escape_log_field(&record.endpoint),
                record.payload.len(),
                content
            ));
        }
        output
    }

    fn evict_oldest(&mut self) {
        if let Some(record) = self.records.pop_front() {
            self.payload_bytes = self.payload_bytes.saturating_sub(record.payload_bytes());
            self.evicted_records = self.evicted_records.saturating_add(1);
        }
    }
}

fn escape_log_field(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(payload: Vec<u8>) -> CommunicationRecord {
        CommunicationRecord {
            timestamp: "12:34:56.789".into(),
            direction: CommunicationDirection::Incoming,
            endpoint: "loopback".into(),
            payload,
            payload_format: None,
            note: None,
        }
    }

    #[test]
    fn text_hex_and_crlf_are_decoded_without_ambiguity() {
        assert_eq!(
            decode_payload("你好", PayloadFormat::Text, false).unwrap(),
            "你好".as_bytes()
        );
        assert_eq!(
            decode_payload("41 0a FF", PayloadFormat::Hex, true).unwrap(),
            b"A\n\xFF\r\n"
        );
        assert!(decode_payload("410A", PayloadFormat::Hex, false).is_err());
        assert!(decode_payload("A", PayloadFormat::Hex, false).is_err());
    }

    #[test]
    fn text_rendering_preserves_printable_unicode_and_escapes_controls() {
        assert_eq!(
            render_payload("中文🙂\\path\r\n\t".as_bytes(), PayloadFormat::Text),
            "中文🙂\\path\\r\\n\\t"
        );
        assert!(!render_payload("中文".as_bytes(), PayloadFormat::Text).contains("\\u{"));
    }

    #[test]
    fn invalid_utf8_is_marked_while_original_bytes_remain_available() {
        let payload = [0x66, 0x80];
        assert!(render_payload(&payload, PayloadFormat::Text).contains("无效 UTF-8"));
        assert_eq!(render_payload(&payload, PayloadFormat::Hex), "66 80");
    }

    #[test]
    fn outgoing_records_keep_the_format_used_when_sent() {
        let outgoing = CommunicationRecord {
            direction: CommunicationDirection::Outgoing,
            payload: "中文".as_bytes().to_vec(),
            payload_format: Some(PayloadFormat::Text),
            ..record(Vec::new())
        };

        assert_eq!(outgoing.display_content(PayloadFormat::Hex), "中文");
        assert_eq!(outgoing.display_format(PayloadFormat::Hex), PayloadFormat::Text);
    }

    #[test]
    fn incoming_records_follow_the_current_receive_format() {
        let incoming = record(b"AB".to_vec());

        assert_eq!(incoming.display_content(PayloadFormat::Text), "AB");
        assert_eq!(incoming.display_content(PayloadFormat::Hex), "41 42");
    }

    #[test]
    fn log_export_uses_each_records_effective_display_format() {
        let mut log = CommunicationLog::default();
        log.push_batch(vec![
            CommunicationRecord {
                direction: CommunicationDirection::Outgoing,
                payload: "发送".as_bytes().to_vec(),
                payload_format: Some(PayloadFormat::Text),
                ..record(Vec::new())
            },
            CommunicationRecord {
                payload: b"RX".to_vec(),
                ..record(Vec::new())
            },
        ]);

        let export = log.export(PayloadFormat::Hex);
        assert!(export.contains("发送"));
        assert!(export.contains("52 58"));
        assert!(!export.contains("\\u{"));
    }

    #[test]
    fn session_summary_preserves_current_tcp_and_serial_configuration() {
        let tcp = CommunicationConfig::Tcp(TcpDebugConfig {
            mode: TcpDebugMode::Client,
            family: CommunicationIpFamily::V4,
            address: "192.0.2.10".into(),
            port: 8080,
            connect_timeout_ms: 3_000,
        });
        let serial = CommunicationConfig::Serial(SerialDebugConfig {
            port_name: "COM3".into(),
            baud_rate: 115_200,
            data_bits: SerialDataBits::Eight,
            parity: SerialParity::None,
            stop_bits: SerialStopBits::One,
            flow_control: SerialFlowControl::None,
            read_timeout_ms: 100,
        });

        assert_eq!(tcp.session_summary(), "192.0.2.10:8080");
        assert_eq!(serial.session_summary(), "COM3，115200，8N1");
    }

    #[test]
    fn log_enforces_record_and_payload_limits_independently() {
        let mut log = CommunicationLog::default();
        for _ in 0..=MAX_LOG_RECORDS {
            log.push_batch(vec![record(vec![1])]);
        }
        assert_eq!(log.records().len(), MAX_LOG_RECORDS);
        assert_eq!(log.evicted_records, 1);

        log.clear();
        log.push_batch(vec![record(vec![0; MAX_LOG_PAYLOAD_BYTES])]);
        log.push_batch(vec![record(vec![1])]);
        assert_eq!(log.records().len(), 1);
        assert_eq!(log.payload_bytes, 1);
    }

    #[test]
    fn queue_drop_counter_is_separate_from_log_eviction() {
        let mut log = CommunicationLog::default();
        log.add_queue_dropped(7);
        assert_eq!(log.queue_dropped_records(), 7);
        assert_eq!(log.evicted_records, 0);
    }

    #[test]
    fn udp_multicast_validation_rejects_non_multicast_and_ipv6_modes() {
        let base = UdpDebugConfig {
            family: CommunicationIpFamily::V4,
            local_address: "0.0.0.0".into(),
            local_port: 9000,
            remote_address: String::new(),
            remote_port: 0,
            broadcast: false,
            multicast: Some(UdpMulticastConfig {
                group: "192.0.2.1".into(),
                interface: "0.0.0.0".into(),
                ttl: 1,
                loopback: true,
            }),
        };
        assert!(base.validate().is_err());
        let mut ipv6 = base;
        ipv6.family = CommunicationIpFamily::V6;
        assert!(ipv6.validate().is_err());
    }

    #[test]
    fn udp_broadcast_and_multicast_boundaries_are_validated() {
        let mut config = UdpDebugConfig {
            family: CommunicationIpFamily::V6,
            local_address: "::".into(),
            local_port: 9000,
            remote_address: String::new(),
            remote_port: 0,
            broadcast: true,
            multicast: None,
        };
        assert!(config.validate().is_err());
        config.family = CommunicationIpFamily::V4;
        config.local_address = "0.0.0.0".into();
        config.broadcast = false;
        config.multicast = Some(UdpMulticastConfig {
            group: "239.255.0.1".into(),
            interface: "0.0.0.0".into(),
            ttl: 255,
            loopback: false,
        });
        assert!(config.validate().is_ok());
        config.multicast.as_mut().unwrap().ttl = 0;
        assert!(config.validate().is_err());
    }
}
