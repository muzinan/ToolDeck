//! TCP 握手测试领域模型。

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TcpProbeAttempt {
    pub address: String,
    pub elapsed_ms: f64,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TcpProbeResult {
    pub host: String,
    pub port: u16,
    pub attempts: Vec<TcpProbeAttempt>,
    pub success: bool,
}
