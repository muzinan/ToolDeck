//! 领域模型与统一错误模型。
//! 平台层负责将 Win32 原始结构转换为本模块的数据类型，UI 不直接接触 FFI 数据。

pub mod communication;
pub mod dns;
pub mod error;
pub mod file;
pub mod mtr;
pub mod network;
pub mod ping;
pub mod process;
pub mod tcp_probe;

pub use communication::{
    COMMUNICATION_QUEUE_CAPACITY, CommunicationCommand, CommunicationConfig,
    CommunicationDirection, CommunicationEvent, CommunicationEventEnvelope, CommunicationIpFamily,
    CommunicationKind, CommunicationLog, CommunicationPeer, CommunicationRecord,
    CommunicationSendTarget, CommunicationSessionState, MAX_RECORD_BATCH, MAX_TCP_PENDING_BYTES,
    MAX_UDP_PAYLOAD_BYTES, PayloadFormat, RECORD_BATCH_INTERVAL_MS, SerialDataBits,
    SerialDebugConfig, SerialFlowControl, SerialParity, SerialPortDescriptor, SerialStopBits,
    TcpDebugConfig, TcpDebugMode, UdpDebugConfig, UdpMulticastConfig, decode_payload,
    render_payload,
};
pub use dns::{DnsRecord, DnsRecordType, DnsResult};
pub use error::AppError;
pub use file::FileLockResult;
pub use mtr::{MtrConfig, MtrHopStats, MtrProgress, MtrResult};
pub use network::{IpVersion, NetworkEndpoint, NetworkProtocol, TcpState};
pub use ping::{PingAddressFamily, PingSample, PingSummary};
pub use process::{
    ProcessCommandLine, ProcessInfo, ProcessSummary, ProcessTreeNode, ProcessTreeSnapshot,
};
pub use tcp_probe::{TcpProbeAttempt, TcpProbeResult};
