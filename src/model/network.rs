//! 网络连接查询的领域模型。

use serde::{Deserialize, Serialize};

/// 端点使用的传输层协议。
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum NetworkProtocol {
    Tcp,
    Udp,
}

impl NetworkProtocol {
    pub fn label(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        }
    }
}

/// 端点使用的 IP 地址版本。
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum IpVersion {
    V4,
    V6,
}

impl IpVersion {
    pub fn label(self) -> &'static str {
        match self {
            Self::V4 => "IPv4",
            Self::V6 => "IPv6",
        }
    }
}

/// TCP 连接状态；UDP 没有连接状态。
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum TcpState {
    Listen,
    Established,
    SynSent,
    SynReceived,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
    Closed,
    DeleteTcb,
    Unknown,
}

impl TcpState {
    pub const ALL: [Self; 13] = [
        Self::Listen,
        Self::Established,
        Self::SynSent,
        Self::SynReceived,
        Self::FinWait1,
        Self::FinWait2,
        Self::CloseWait,
        Self::Closing,
        Self::LastAck,
        Self::TimeWait,
        Self::Closed,
        Self::DeleteTcb,
        Self::Unknown,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Listen => "LISTENING",
            Self::Established => "ESTABLISHED",
            Self::SynSent => "SYN_SENT",
            Self::SynReceived => "SYN_RECEIVED",
            Self::FinWait1 => "FIN_WAIT_1",
            Self::FinWait2 => "FIN_WAIT_2",
            Self::CloseWait => "CLOSE_WAIT",
            Self::Closing => "CLOSING",
            Self::LastAck => "LAST_ACK",
            Self::TimeWait => "TIME_WAIT",
            Self::Closed => "CLOSED",
            Self::DeleteTcb => "DELETE_TCB",
            Self::Unknown => "UNKNOWN",
        }
    }
}

/// 一个由 Windows 网络表返回并经过格式化的网络端点。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NetworkEndpoint {
    /// TCP 或 UDP，由 IP Helper API 的表类型决定。
    pub protocol: NetworkProtocol,
    /// IPv4 或 IPv6，由查询地址族决定。
    pub ip_version: IpVersion,
    /// 已格式化的本地地址，不含端口。
    pub local_address: String,
    /// 本地端口号，来自 IP Helper 的网络字节序字段。
    pub local_port: u16,
    /// 已格式化的远端地址；监听端口使用空字符串。
    pub remote_address: String,
    /// 远端端口号；UDP 和监听端口为 `None`。
    pub remote_port: Option<u16>,
    /// TCP 状态；UDP 为 `None`。
    pub state: Option<TcpState>,
    /// 创建端点的进程 PID。
    pub pid: u32,
    /// 进程快照中解析出的可执行文件名；进程退出时为空。
    pub process_name: String,
}

impl NetworkEndpoint {
    pub fn local_display(&self) -> String {
        format_endpoint(self.ip_version, &self.local_address, self.local_port)
    }

    pub fn remote_display(&self) -> String {
        match self.remote_port {
            Some(port) => format_endpoint(self.ip_version, &self.remote_address, port),
            None => "-".to_owned(),
        }
    }
}

fn format_endpoint(version: IpVersion, address: &str, port: u16) -> String {
    match version {
        IpVersion::V4 => format!("{address}:{port}"),
        IpVersion::V6 => format!("[{address}]:{port}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{IpVersion, NetworkEndpoint, NetworkProtocol};

    #[test]
    fn ipv6_endpoint_uses_brackets_around_address() {
        let endpoint = NetworkEndpoint {
            protocol: NetworkProtocol::Tcp,
            ip_version: IpVersion::V6,
            local_address: "::1".into(),
            local_port: 443,
            remote_address: "2001:db8::1".into(),
            remote_port: Some(50_000),
            state: None,
            pid: 1,
            process_name: String::new(),
        };
        assert_eq!(endpoint.local_display(), "[::1]:443");
        assert_eq!(endpoint.remote_display(), "[2001:db8::1]:50000");
    }
}
