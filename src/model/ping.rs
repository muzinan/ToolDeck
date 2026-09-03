//! Ping 查询领域模型。

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub enum PingAddressFamily {
    #[default]
    Auto,
    V4,
    V6,
}

impl PingAddressFamily {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动",
            Self::V4 => "IPv4",
            Self::V6 => "IPv6",
        }
    }
}

/// Ping 任务配置；持续模式由取消令牌结束，有限模式执行 `count` 次。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PingConfig {
    pub family: PingAddressFamily,
    pub count: u32,
    pub timeout_ms: u32,
    pub payload_size: u16,
    pub interval_ms: u32,
    pub continuous: bool,
}

impl Default for PingConfig {
    fn default() -> Self {
        Self {
            family: PingAddressFamily::Auto,
            count: 4,
            timeout_ms: 1_000,
            payload_size: 32,
            interval_ms: 1_000,
            continuous: false,
        }
    }
}

impl PingConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=100).contains(&self.count) {
            return Err("有限模式次数必须在 1–100 之间。".into());
        }
        if !(100..=10_000).contains(&self.timeout_ms) {
            return Err("单次超时必须在 100–10000 ms 之间。".into());
        }
        if self.payload_size > 1_472 {
            return Err("ICMP 载荷必须在 0–1472 字节之间。".into());
        }
        if !(100..=60_000).contains(&self.interval_ms) {
            return Err("探测间隔必须在 100–60000 ms 之间。".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PingSample {
    pub address: String,
    pub elapsed_ms: Option<f64>,
    pub ttl: Option<u8>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PingSummary {
    pub host: String,
    pub samples: Vec<PingSample>,
    pub sent: u32,
    pub received: u32,
    pub min_ms: Option<f64>,
    pub avg_ms: Option<f64>,
    pub max_ms: Option<f64>,
}

/// 单个 ICMP 样本完成后的实时聚合快照。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct PingProgress {
    pub host: String,
    pub sample: PingSample,
    pub sent: u32,
    pub received: u32,
    pub min_ms: Option<f64>,
    pub avg_ms: Option<f64>,
    pub max_ms: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::PingConfig;

    #[test]
    fn ping_config_validates_finite_and_continuous_boundaries() {
        let mut config = PingConfig::default();
        assert!(config.validate().is_ok());
        config.count = 0;
        assert!(config.validate().is_err());
        config = PingConfig::default();
        config.interval_ms = 99;
        assert!(config.validate().is_err());
        config = PingConfig::default();
        config.continuous = true;
        assert!(config.validate().is_ok());
    }
}
