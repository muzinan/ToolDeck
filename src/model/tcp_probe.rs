//! TCP 握手测试领域模型。

use serde::{Deserialize, Serialize};

use super::PingAddressFamily;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TcpProbeConfig {
    pub family: PingAddressFamily,
    pub timeout_ms: u32,
    pub attempts: u32,
    pub interval_ms: u32,
}

impl Default for TcpProbeConfig {
    fn default() -> Self {
        Self {
            family: PingAddressFamily::Auto,
            timeout_ms: 3_000,
            attempts: 3,
            interval_ms: 1_000,
        }
    }
}

impl TcpProbeConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !(100..=30_000).contains(&self.timeout_ms) {
            return Err("TCP 超时必须在 100–30000 ms 之间。".into());
        }
        if !(1..=100).contains(&self.attempts) {
            return Err("尝试次数必须在 1–100 之间。".into());
        }
        if !(100..=60_000).contains(&self.interval_ms) {
            return Err("尝试间隔必须在 100–60000 ms 之间。".into());
        }
        Ok(())
    }
}

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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TcpProbeProgress {
    pub attempt_number: u32,
    pub attempt: TcpProbeAttempt,
}
