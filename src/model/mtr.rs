//! MTR 路径诊断的领域模型与边界校验。

use serde::{Deserialize, Serialize};

use super::PingAddressFamily;

/// MTR 高级探测参数；`total_rounds = None` 表示持续运行直到用户停止。
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MtrConfig {
    pub max_hops: u8,
    pub probes_per_hop: u8,
    pub timeout_ms: u32,
    pub interval_ms: u32,
    pub payload_size: u16,
    pub total_rounds: Option<u32>,
    pub concurrency: u8,
    pub resolve_hostnames: bool,
    pub family: PingAddressFamily,
}

impl Default for MtrConfig {
    fn default() -> Self {
        Self {
            max_hops: 30,
            probes_per_hop: 3,
            timeout_ms: 1_000,
            interval_ms: 1_000,
            payload_size: 32,
            total_rounds: None,
            concurrency: 1,
            resolve_hostnames: false,
            family: PingAddressFamily::Auto,
        }
    }
}

impl MtrConfig {
    /// 校验界面输入，返回第一条可行动的修正提示。
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=64).contains(&self.max_hops) {
            return Err("最大跳数必须在 1 到 64 之间。".into());
        }
        if !(1..=10).contains(&self.probes_per_hop) {
            return Err("每跳探测次数必须在 1 到 10 之间。".into());
        }
        if !(100..=5_000).contains(&self.timeout_ms) {
            return Err("单次超时必须在 100 到 5000 毫秒之间。".into());
        }
        if !(200..=5_000).contains(&self.interval_ms) {
            return Err("探测间隔必须在 200 到 5000 毫秒之间。".into());
        }
        if self.payload_size > 1_472 {
            return Err("ICMP 载荷必须在 0 到 1472 字节之间。".into());
        }
        if self
            .total_rounds
            .is_some_and(|rounds| !(1..=100).contains(&rounds))
        {
            return Err("总轮数必须在 1 到 100 之间。".into());
        }
        if !(1..=4).contains(&self.concurrency) {
            return Err("最大并发探测数必须在 1 到 4 之间。".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MtrHopStats {
    pub hop: u8,
    pub address: Option<String>,
    pub hostname: Option<String>,
    pub sent: u32,
    pub received: u32,
    pub min_ms: Option<f64>,
    pub avg_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub last_ms: Option<f64>,
    pub jitter_ms: Option<f64>,
    pub status: String,
}

impl MtrHopStats {
    pub fn new(hop: u8) -> Self {
        Self {
            hop,
            address: None,
            hostname: None,
            sent: 0,
            received: 0,
            min_ms: None,
            avg_ms: None,
            max_ms: None,
            last_ms: None,
            jitter_ms: None,
            status: "等待探测".into(),
        }
    }

    pub fn loss_percent(&self) -> f64 {
        if self.sent == 0 {
            0.0
        } else {
            100.0 * f64::from(self.sent - self.received) / f64::from(self.sent)
        }
    }

    pub fn record(&mut self, address: Option<String>, elapsed_ms: Option<f64>, status: &str) {
        self.sent += 1;
        self.status = status.to_owned();
        if let Some(address) = address {
            if self.address.as_deref() != Some(address.as_str()) {
                self.hostname = None;
            }
            self.address = Some(address);
        }
        let Some(elapsed_ms) = elapsed_ms else {
            return;
        };
        let previous_last = self.last_ms;
        self.received += 1;
        self.last_ms = Some(elapsed_ms);
        self.min_ms = Some(
            self.min_ms
                .map_or(elapsed_ms, |value| value.min(elapsed_ms)),
        );
        self.max_ms = Some(
            self.max_ms
                .map_or(elapsed_ms, |value| value.max(elapsed_ms)),
        );
        let previous = self.avg_ms.unwrap_or(0.0) * f64::from(self.received - 1);
        self.avg_ms = Some((previous + elapsed_ms) / f64::from(self.received));
        if let Some(previous_last) = previous_last {
            let delta = (elapsed_ms - previous_last).abs();
            let transition_count = self.received - 1;
            let previous_total = self.jitter_ms.unwrap_or(0.0) * f64::from(transition_count - 1);
            self.jitter_ms = Some((previous_total + delta) / f64::from(transition_count));
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MtrProgress {
    pub host: String,
    pub round: u32,
    pub hops: Vec<MtrHopStats>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MtrResult {
    pub host: String,
    pub rounds: u32,
    pub stopped: bool,
    pub hops: Vec<MtrHopStats>,
}

#[cfg(test)]
mod tests {
    use super::{MtrConfig, MtrHopStats};

    #[test]
    fn default_config_matches_bounded_probe_defaults() {
        let config = MtrConfig::default();
        assert_eq!(config.max_hops, 30);
        assert_eq!(config.probes_per_hop, 3);
        assert!(config.total_rounds.is_none());
        assert!(config.validate().is_ok());
    }

    #[test]
    fn config_rejects_each_out_of_range_boundary() {
        let mut config = MtrConfig {
            max_hops: 0,
            ..MtrConfig::default()
        };
        assert!(config.validate().is_err());
        config = MtrConfig::default();
        config.timeout_ms = 5_001;
        assert!(config.validate().is_err());
        config = MtrConfig::default();
        config.payload_size = 1_473;
        assert!(config.validate().is_err());
        config = MtrConfig::default();
        config.total_rounds = Some(101);
        assert!(config.validate().is_err());
    }

    #[test]
    fn hop_statistics_track_loss_and_latency() {
        let mut hop = MtrHopStats::new(1);
        hop.record(Some("192.0.2.1".into()), Some(10.0), "响应");
        hop.record(None, None, "超时");
        hop.record(None, Some(20.0), "响应");
        assert_eq!(hop.sent, 3);
        assert_eq!(hop.received, 2);
        assert!((hop.loss_percent() - 33.333).abs() < 0.01);
        assert_eq!(hop.avg_ms, Some(15.0));
        assert_eq!(hop.jitter_ms, Some(10.0));
    }

    #[test]
    fn hop_address_change_clears_stale_hostname() {
        let mut hop = MtrHopStats::new(1);
        hop.record(Some("192.0.2.1".into()), Some(10.0), "中间跳");
        hop.hostname = Some("router-a.example".into());

        hop.record(Some("192.0.2.2".into()), Some(12.0), "中间跳");

        assert_eq!(hop.address.as_deref(), Some("192.0.2.2"));
        assert!(hop.hostname.is_none());
    }

    #[test]
    fn separate_hops_keep_independent_statistics_for_repeated_addresses() {
        let mut first = MtrHopStats::new(1);
        let mut second = MtrHopStats::new(2);

        first.record(Some("192.0.2.1".into()), Some(10.0), "中间跳");
        second.record(Some("192.0.2.1".into()), None, "无响应");

        assert_eq!(first.received, 1);
        assert_eq!(second.received, 0);
        assert_eq!(first.address, second.address);
    }
}
