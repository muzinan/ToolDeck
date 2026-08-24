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
