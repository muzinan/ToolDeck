//! DNS、Ping、TCP 握手测试工具页面。

use crate::{
    core::{
        actions::AppAction,
        invocation::{ToolInvocation, ToolPayload},
        worker::TaskResult,
    },
    model::{DnsRecordType, PingAddressFamily},
    tools::{
        ToolModule, ToolUiContext,
        registry::{ToolCategory, ToolDescriptor, ToolIcon},
    },
    ui,
};
use eframe::egui::{self, RichText, TextEdit};
use std::time::Instant;

#[derive(Default)]
pub struct DnsLookupTool {
    host: String,
    record_type: DnsRecordType,
    bypass_cache: bool,
    busy: bool,
    result: Option<Result<crate::model::DnsResult, crate::model::AppError>>,
}
pub struct PingTool {
    host: String,
    count: u32,
    timeout_ms: u32,
    payload_size: u16,
    family: PingAddressFamily,
    busy: bool,
    progress: Vec<String>,
    result: Option<Result<crate::model::PingSummary, crate::model::AppError>>,
}
pub struct TcpProbeTool {
    host: String,
    port: u16,
    timeout_ms: u32,
    busy: bool,
    result: Option<Result<crate::model::TcpProbeResult, crate::model::AppError>>,
}

impl Default for PingTool {
    fn default() -> Self {
        Self {
            host: String::new(),
            count: 4,
            timeout_ms: 1_000,
            payload_size: 32,
            family: PingAddressFamily::Auto,
            busy: false,
            progress: Vec::new(),
            result: None,
        }
    }
}

impl Default for TcpProbeTool {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 443,
            timeout_ms: 3_000,
            busy: false,
            result: None,
        }
    }
}

fn input_row(ui: &mut egui::Ui, host: &mut String, button: &str, busy: bool) -> bool {
    let mut run = false;
    let available_width = ui.available_width();
    match ui::action_layout(available_width) {
        ui::ActionLayout::Horizontal => ui.horizontal(|ui| {
            let input_width = (available_width - 112.0).max(180.0);
            ui.add_sized(
                [input_width, 34.0],
                TextEdit::singleline(host).hint_text("输入主机名或 IP"),
            );
            if ui::primary_button(ui, if busy { "重新查询" } else { button }).clicked() {
                run = true;
            }
        }),
        ui::ActionLayout::Vertical => ui.vertical(|ui| {
            ui.add_sized(
                [ui.available_width(), 34.0],
                TextEdit::singleline(host).hint_text("输入主机名或 IP"),
            );
            if ui::primary_button(ui, if busy { "重新查询" } else { button }).clicked() {
                run = true;
            }
        }),
    };
    run
}
fn state(ui: &mut egui::Ui, busy: bool, error: Option<&crate::model::AppError>) {
    if busy {
        ui.spinner();
        ui.label("正在查询...");
    }
    if let Some(error) = error {
        let (title, detail) = error.user_message();
        ui::state_card(ui, title, &detail, ui::danger_text(ui));
    }
}

impl ToolModule for DnsLookupTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: "dns-lookup",
            name: "DNS 查询",
            description: "查询 A、AAAA、CNAME、MX、TXT、NS 与 PTR 记录",
            category: ToolCategory::Network,
            icon: ToolIcon::Dns,
            keywords: &["dns", "resolve", "域名", "解析"],
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _context: ToolUiContext) -> Vec<AppAction> {
        let mut actions = Vec::new();
        ui::page_heading(ui, "DNS 查询", "使用系统 DNS 解析主机名和记录。 ");
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            if input_row(ui, &mut self.host, "查询", self.busy) {
                actions.push(AppAction::RunDns {
                    host: self.host.clone(),
                    record_type: self.record_type,
                    bypass_cache: self.bypass_cache,
                });
            }
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("dns-type")
                    .selected_text(self.record_type.label())
                    .show_ui(ui, |ui| {
                        for value in DnsRecordType::ALL {
                            ui.selectable_value(&mut self.record_type, value, value.label());
                        }
                    });
                ui.checkbox(&mut self.bypass_cache, "绕过缓存（系统解析器不一定支持）");
            });
        });
        ui.add_space(ui::SPACE_16);
        match &self.result {
            Some(Ok(result)) => {
                ui::card(ui, |ui| {
                    ui.label(RichText::new(format!("{} 条记录", result.records.len())).strong());
                    for record in &result.records {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(&record.name);
                            ui.label(record.record_type.label());
                            ui.label(&record.value).on_hover_text(&record.value);
                            ui.label(format!("TTL {}", record.ttl));
                        });
                    }
                    ui.horizontal_wrapped(|ui| {
                        if ui.button("Ping 此主机").clicked() {
                            actions.push(AppAction::InvokeTool(ToolInvocation::host(
                                "ping",
                                result.host.clone(),
                            )));
                        }
                        if ui.button("TCP 测试 443").clicked() {
                            actions.push(AppAction::InvokeTool(ToolInvocation::tcp_probe(
                                result.host.clone(),
                                443,
                            )));
                        }
                    });
                });
            }
            Some(Err(error)) => state(ui, self.busy, Some(error)),
            None => state(ui, self.busy, None),
        }
        actions
    }
    fn handle_invocation(&mut self, payload: ToolPayload) -> Vec<AppAction> {
        if let ToolPayload::Host { host } = payload {
            self.host = host;
            return vec![AppAction::RunDns {
                host: self.host.clone(),
                record_type: self.record_type,
                bypass_cache: self.bypass_cache,
            }];
        }
        Vec::new()
    }
    fn handle_task_result(&mut self, result: TaskResult) {
        if let TaskResult::Dns(result) = result {
            self.busy = false;
            self.result = Some(result);
        }
    }
    fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }
    fn poll_actions(&mut self, _now: Instant) -> Vec<AppAction> {
        Vec::new()
    }
}

impl ToolModule for PingTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: "ping",
            name: "Ping",
            description: "使用系统网络接口测试主机响应和延迟",
            category: ToolCategory::Network,
            icon: ToolIcon::Ping,
            keywords: &["ping", "icmp", "延迟", "连通性"],
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _context: ToolUiContext) -> Vec<AppAction> {
        let mut actions = Vec::new();
        ui::page_heading(ui, "Ping", "测试 IPv4 / IPv6 响应和往返延迟。");
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            if input_row(ui, &mut self.host, "开始", self.busy) {
                actions.push(AppAction::RunPing {
                    host: self.host.clone(),
                    count: self.count.clamp(1, 10),
                    timeout_ms: self.timeout_ms.clamp(100, 10000),
                    payload_size: self.payload_size.min(1472),
                    family: self.family,
                });
            }
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::DragValue::new(&mut self.count)
                        .range(1..=10)
                        .prefix("次数 "),
                );
                ui.add(
                    egui::DragValue::new(&mut self.timeout_ms)
                        .range(100..=10000)
                        .prefix("超时 ")
                        .suffix(" ms"),
                );
                ui.add(
                    egui::DragValue::new(&mut self.payload_size)
                        .range(0..=1472)
                        .prefix("载荷 "),
                );
                egui::ComboBox::from_id_salt("ping-family")
                    .selected_text(self.family.label())
                    .show_ui(ui, |ui| {
                        for value in [
                            PingAddressFamily::Auto,
                            PingAddressFamily::V4,
                            PingAddressFamily::V6,
                        ] {
                            ui.selectable_value(&mut self.family, value, value.label());
                        }
                    });
            });
        });
        ui.add_space(ui::SPACE_16);
        if self.busy && !self.progress.is_empty() {
            ui::card(ui, |ui| {
                ui.label(RichText::new("实时响应").strong());
                for message in &self.progress {
                    ui.label(message);
                }
            });
            ui.add_space(ui::SPACE_16);
        }
        match &self.result {
            Some(Ok(result)) => {
                ui::card(ui, |ui| {
                    ui.label(format!(
                        "发送 {}，接收 {}，丢包 {:.1}%",
                        result.sent,
                        result.received,
                        if result.sent == 0 {
                            0.0
                        } else {
                            100.0 - result.received as f64 * 100.0 / result.sent as f64
                        }
                    ));
                    for sample in &result.samples {
                        let detail = sample.elapsed_ms.map_or_else(
                            || sample.error.clone().unwrap_or_else(|| "失败".into()),
                            |ms| format!("{ms:.1} ms"),
                        );
                        let ttl = sample
                            .ttl
                            .map_or_else(String::new, |value| format!(" · TTL {value}"));
                        ui.label(format!("{}  {}{}", sample.address, detail, ttl,));
                    }
                    ui.label(format!(
                        "最小 {} · 平均 {} · 最大 {}",
                        result
                            .min_ms
                            .map_or_else(|| "-".into(), |value| format!("{value:.1} ms")),
                        result
                            .avg_ms
                            .map_or_else(|| "-".into(), |value| format!("{value:.1} ms")),
                        result
                            .max_ms
                            .map_or_else(|| "-".into(), |value| format!("{value:.1} ms")),
                    ));
                    if ui.button("TCP 测试 443").clicked() {
                        actions.push(AppAction::InvokeTool(ToolInvocation::tcp_probe(
                            result.host.clone(),
                            443,
                        )));
                    }
                });
            }
            Some(Err(error)) => state(ui, self.busy, Some(error)),
            None => state(ui, self.busy, None),
        }
        actions
    }
    fn handle_invocation(&mut self, payload: ToolPayload) -> Vec<AppAction> {
        if let ToolPayload::Host { host } = payload {
            self.host = host;
            return vec![AppAction::RunPing {
                host: self.host.clone(),
                count: self.count.clamp(1, 10),
                timeout_ms: self.timeout_ms.clamp(100, 10000),
                payload_size: self.payload_size.min(1472),
                family: self.family,
            }];
        }
        Vec::new()
    }
    fn handle_task_result(&mut self, result: TaskResult) {
        if let TaskResult::Ping(result) = result {
            self.busy = false;
            self.result = Some(result);
        }
    }
    fn handle_task_progress(
        &mut self,
        message: String,
        _completed: Option<u64>,
        _total: Option<u64>,
    ) {
        self.progress.push(message);
        self.progress.truncate(10);
    }
    fn set_busy(&mut self, busy: bool) {
        if busy {
            self.progress.clear();
        }
        self.busy = busy;
    }
    fn poll_actions(&mut self, _now: Instant) -> Vec<AppAction> {
        Vec::new()
    }
}

impl ToolModule for TcpProbeTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: "tcp-probe",
            name: "TCP 端口测试",
            description: "在截止时间内测试 TCP 握手是否成功",
            category: ToolCategory::Network,
            icon: ToolIcon::TcpProbe,
            keywords: &["tcp", "probe", "connect", "握手"],
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _context: ToolUiContext) -> Vec<AppAction> {
        let mut actions = Vec::new();
        ui::page_heading(ui, "TCP 端口测试", "只执行 TCP 握手，不发送应用数据。");
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            let available_width = ui.available_width();
            match ui::action_layout(available_width) {
                ui::ActionLayout::Horizontal => {
                    ui.horizontal(|ui| {
                        let host_width = (available_width - 260.0).max(180.0);
                        ui.add_sized(
                            [host_width, 34.0],
                            TextEdit::singleline(&mut self.host).hint_text("主机名或 IP"),
                        );
                        tcp_controls(
                            ui,
                            &mut self.port,
                            &mut self.timeout_ms,
                            &mut actions,
                            &self.host,
                        );
                    });
                }
                ui::ActionLayout::Vertical => {
                    ui.add_sized(
                        [ui.available_width(), 34.0],
                        TextEdit::singleline(&mut self.host).hint_text("主机名或 IP"),
                    );
                    ui.add_space(ui::SPACE_8);
                    ui.horizontal_wrapped(|ui| {
                        tcp_controls(
                            ui,
                            &mut self.port,
                            &mut self.timeout_ms,
                            &mut actions,
                            &self.host,
                        );
                    });
                }
            }
        });
        ui.add_space(ui::SPACE_16);
        match &self.result {
            Some(Ok(result)) => ui::card(ui, |ui| {
                ui.label(if result.success {
                    RichText::new("连接成功").color(ui::success_text(ui))
                } else {
                    RichText::new("连接失败").color(ui::danger_text(ui))
                });
                for attempt in &result.attempts {
                    ui.label(format!(
                        "{}  {:.1} ms  {}",
                        attempt.address, attempt.elapsed_ms, attempt.status
                    ));
                }
            }),
            Some(Err(error)) => state(ui, self.busy, Some(error)),
            None => state(ui, self.busy, None),
        }
        actions
    }
    fn handle_invocation(&mut self, payload: ToolPayload) -> Vec<AppAction> {
        if let ToolPayload::HostPort { host, port } = payload {
            self.host = host;
            self.port = port;
            return vec![AppAction::RunTcpProbe {
                host: self.host.clone(),
                port,
                timeout_ms: self.timeout_ms.clamp(100, 30000),
            }];
        }
        Vec::new()
    }
    fn handle_task_result(&mut self, result: TaskResult) {
        if let TaskResult::TcpProbe(result) = result {
            self.busy = false;
            self.result = Some(result);
        }
    }
    fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }
    fn poll_actions(&mut self, _now: Instant) -> Vec<AppAction> {
        Vec::new()
    }
}

fn tcp_controls(
    ui: &mut egui::Ui,
    port: &mut u16,
    timeout_ms: &mut u32,
    actions: &mut Vec<AppAction>,
    host: &str,
) {
    ui.add(egui::DragValue::new(port).range(1..=65535).prefix("端口 "));
    ui.add(
        egui::DragValue::new(timeout_ms)
            .range(100..=30000)
            .suffix(" ms"),
    );
    if ui::primary_button(ui, "测试").clicked() {
        actions.push(AppAction::RunTcpProbe {
            host: host.to_owned(),
            port: *port,
            timeout_ms: (*timeout_ms).clamp(100, 30000),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{PingTool, TcpProbeTool};

    #[test]
    fn network_tool_defaults_match_the_documented_probe_values() {
        let ping = PingTool::default();
        assert_eq!(ping.count, 4);
        assert_eq!(ping.timeout_ms, 1_000);
        assert_eq!(ping.payload_size, 32);

        let tcp = TcpProbeTool::default();
        assert_eq!(tcp.port, 443);
        assert_eq!(tcp.timeout_ms, 3_000);
    }
}
