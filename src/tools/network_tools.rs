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
use eframe::egui::{self, RichText};
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
        ui::ActionLayout::Horizontal => {
            ui.horizontal(|ui| {
                let button_width = 96.0;
                let input_width =
                    (available_width - button_width - ui.spacing().item_spacing.x).max(180.0);
                let input = ui.add_sized(
                    [input_width, ui::CONTROL_HEIGHT],
                    ui::text_input(host, "输入域名或 IP (例如 www.bing.com 或 1.1.1.1)"),
                );
                run |= input.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if ui::primary_button_sized(
                    ui,
                    if busy { "重新查询" } else { button },
                    [button_width, ui::CONTROL_HEIGHT],
                )
                .clicked()
                {
                    run = true;
                }
            });
        }
        ui::ActionLayout::Vertical => {
            ui.vertical(|ui| {
                let input = ui.add_sized(
                    [ui.available_width(), ui::CONTROL_HEIGHT],
                    ui::text_input(host, "输入域名或 IP (例如 www.bing.com 或 1.1.1.1)"),
                );
                let submit =
                    input.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                ui.add_space(ui::SPACE_8);
                if ui::primary_button(ui, if busy { "重新查询" } else { button }).clicked()
                    || submit
                {
                    run = true;
                }
            });
        }
    };
    run
}

fn state(ui: &mut egui::Ui, busy: bool, error: Option<&crate::model::AppError>) {
    if busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new("正在发起网络查询与解析...").size(13.5));
        });
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
            description: "原生解析 A、AAAA、CNAME、MX、TXT、NS 与 PTR 记录",
            category: ToolCategory::Network,
            icon: ToolIcon::Dns,
            keywords: &["dns", "resolve", "域名", "解析", "nslookup", "ip"],
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _context: ToolUiContext) -> Vec<AppAction> {
        let mut actions = Vec::new();
        ui::page_heading(
            ui,
            "DNS 查询",
            "使用 Windows 原生 DNS 解析机制查询各类型资源记录。",
        );
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            if input_row(ui, &mut self.host, "查询解析", self.busy) {
                actions.push(AppAction::RunDns {
                    host: self.host.clone(),
                    record_type: self.record_type,
                    bypass_cache: self.bypass_cache,
                });
            }
            ui.add_space(ui::SPACE_8);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("记录类型：").color(ui.visuals().weak_text_color()));
                egui::ComboBox::from_id_salt("dns-type")
                    .selected_text(self.record_type.label())
                    .show_ui(ui, |ui| {
                        for value in DnsRecordType::ALL {
                            ui.selectable_value(&mut self.record_type, value, value.label());
                        }
                    });
                ui.add_space(ui::SPACE_12);
                ui.checkbox(&mut self.bypass_cache, "绕过本机 DNS 缓存");
            });
        });
        ui.add_space(ui::SPACE_16);
        match &self.result {
            Some(Ok(result)) => {
                let palette = ui::palette_for_ui(ui);
                ui::card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("查询结果：共 {} 条记录", result.records.len()))
                                .strong()
                                .size(14.5),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui::small_action_button(ui, "复制全部记录").clicked() {
                                let lines: Vec<String> = result
                                    .records
                                    .iter()
                                    .map(|r| {
                                        format!(
                                            "{} {} {} (TTL {})",
                                            r.name,
                                            r.record_type.label(),
                                            r.value,
                                            r.ttl
                                        )
                                    })
                                    .collect();
                                actions.push(AppAction::CopyText(lines.join("\n")));
                            }
                        });
                    });
                    ui.add_space(ui::SPACE_8);
                    for record in &result.records {
                        ui.horizontal(|ui| {
                            let type_str = record.record_type.label();
                            ui::badge(
                                ui,
                                type_str,
                                palette.accent,
                                palette.accent.gamma_multiply(if ui.visuals().dark_mode {
                                    0.22
                                } else {
                                    0.12
                                }),
                            );
                            ui.add_space(ui::SPACE_4);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&record.name).color(ui.visuals().text_color()),
                                )
                                .wrap(),
                            );
                            ui.colored_label(palette.weak, "➔");
                            ui.add(
                                egui::Label::new(RichText::new(&record.value).monospace().strong())
                                    .wrap(),
                            )
                            .on_hover_text(&record.value);
                            ui.colored_label(palette.weak, format!("(TTL {})", record.ttl));
                        });
                        ui.add_space(2.0);
                    }
                    ui.add_space(ui::SPACE_12);
                    ui.separator();
                    ui.add_space(ui::SPACE_8);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("关联诊断：").color(ui.visuals().weak_text_color()));
                        if ui::secondary_button(ui, "Ping 此主机").clicked() {
                            actions.push(AppAction::InvokeTool(ToolInvocation::host(
                                "ping",
                                result.host.clone(),
                            )));
                        }
                        if ui::secondary_button(ui, "TCP 测试 443").clicked() {
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
            description: "通过真实 ICMP 测试主机往返延迟与丢包率",
            category: ToolCategory::Network,
            icon: ToolIcon::Ping,
            keywords: &["ping", "icmp", "延迟", "连通性", "网络质量"],
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _context: ToolUiContext) -> Vec<AppAction> {
        let mut actions = Vec::new();
        ui::page_heading(
            ui,
            "Ping 测试",
            "发送 ICMP Echo 数据包测量网络往返时延与稳定性。",
        );
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            if input_row(ui, &mut self.host, "开始 Ping", self.busy) {
                actions.push(AppAction::RunPing {
                    host: self.host.clone(),
                    count: self.count.clamp(1, 10),
                    timeout_ms: self.timeout_ms.clamp(100, 10000),
                    payload_size: self.payload_size.min(1472),
                    family: self.family,
                });
            }
            ui.add_space(ui::SPACE_8);
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::DragValue::new(&mut self.count)
                        .range(1..=10)
                        .prefix("测试次数: "),
                );
                ui.add_space(ui::SPACE_8);
                ui.add(
                    egui::DragValue::new(&mut self.timeout_ms)
                        .range(100..=10000)
                        .prefix("单次超时: ")
                        .suffix(" ms"),
                );
                ui.add_space(ui::SPACE_8);
                ui.add(
                    egui::DragValue::new(&mut self.payload_size)
                        .range(0..=1472)
                        .prefix("数据载荷: ")
                        .suffix(" B"),
                );
                ui.add_space(ui::SPACE_8);
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
                ui.label(RichText::new("测试进度...").strong());
                ui.add_space(ui::SPACE_4);
                for message in &self.progress {
                    ui.label(RichText::new(message).monospace().size(12.5));
                }
            });
            ui.add_space(ui::SPACE_16);
        }
        match &self.result {
            Some(Ok(result)) => {
                let palette = ui::palette_for_ui(ui);
                ui::card(ui, |ui| {
                    let loss_percent = if result.sent == 0 {
                        0.0
                    } else {
                        100.0 - result.received as f64 * 100.0 / result.sent as f64
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Ping 统计结果").strong().size(15.0));
                        ui.add_space(ui::SPACE_8);
                        let (loss_color, loss_bg) =
                            if loss_percent == 0.0 {
                                (
                                    palette.success_text,
                                    palette.success_text.gamma_multiply(
                                        if ui.visuals().dark_mode { 0.22 } else { 0.12 },
                                    ),
                                )
                            } else if loss_percent < 50.0 {
                                (
                                    palette.warning_text,
                                    palette.warning_text.gamma_multiply(
                                        if ui.visuals().dark_mode { 0.22 } else { 0.12 },
                                    ),
                                )
                            } else {
                                (
                                    palette.danger_text,
                                    palette
                                        .danger_text
                                        .gamma_multiply(if ui.visuals().dark_mode {
                                            0.22
                                        } else {
                                            0.12
                                        }),
                                )
                            };
                        ui::badge(
                            ui,
                            &format!(
                                "丢包率: {:.1}% ({}/{})",
                                loss_percent,
                                result.sent - result.received,
                                result.sent
                            ),
                            loss_color,
                            loss_bg,
                        );
                        if let Some(avg) = result.avg_ms {
                            ui::badge(
                                ui,
                                &format!("平均延迟: {:.1} ms", avg),
                                palette.accent,
                                palette.accent.gamma_multiply(if ui.visuals().dark_mode {
                                    0.22
                                } else {
                                    0.12
                                }),
                            );
                        }
                    });
                    ui.add_space(ui::SPACE_12);
                    for sample in &result.samples {
                        let detail = sample.elapsed_ms.map_or_else(
                            || sample.error.clone().unwrap_or_else(|| "超时/失败".into()),
                            |ms| format!("{ms:.1} ms"),
                        );
                        let ttl = sample
                            .ttl
                            .map_or_else(String::new, |value| format!("  TTL={value}"));
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&sample.address).monospace());
                            ui.label(RichText::new(format!("➔  {detail}{ttl}")).strong().color(
                                if sample.elapsed_ms.is_some() {
                                    palette.success_text
                                } else {
                                    palette.danger_text
                                },
                            ));
                        });
                        ui.add_space(1.0);
                    }
                    ui.add_space(ui::SPACE_12);
                    ui.separator();
                    ui.add_space(ui::SPACE_8);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "往返时延：最小 {} · 平均 {} · 最大 {}",
                                result
                                    .min_ms
                                    .map_or_else(|| "-".into(), |v| format!("{v:.1} ms")),
                                result
                                    .avg_ms
                                    .map_or_else(|| "-".into(), |v| format!("{v:.1} ms")),
                                result
                                    .max_ms
                                    .map_or_else(|| "-".into(), |v| format!("{v:.1} ms")),
                            ))
                            .size(13.0)
                            .color(ui.visuals().weak_text_color()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui::secondary_button(ui, "TCP 测试 443").clicked() {
                                actions.push(AppAction::InvokeTool(ToolInvocation::tcp_probe(
                                    result.host.clone(),
                                    443,
                                )));
                            }
                        });
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
            description: "测试远程主机指定端口的 TCP 三次握手连通性与时延",
            category: ToolCategory::Network,
            icon: ToolIcon::TcpProbe,
            keywords: &["tcp", "probe", "connect", "握手", "端口测试", "连通性"],
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _context: ToolUiContext) -> Vec<AppAction> {
        let mut actions = Vec::new();
        ui::page_heading(
            ui,
            "TCP 端口测试",
            "发起轻量 TCP 三次握手测试端口连通性，不发送应用层负载。",
        );
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            let available_width = ui.available_width();
            match ui::action_layout(available_width) {
                ui::ActionLayout::Horizontal => {
                    ui.horizontal(|ui| {
                        let host_width = (available_width - 280.0).max(180.0);
                        let input = ui.add_sized(
                            [host_width, ui::CONTROL_HEIGHT],
                            ui::text_input(&mut self.host, "主机名或 IP 地址 (如 www.bing.com)"),
                        );
                        let submit = input.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        if tcp_controls(
                            ui,
                            &mut self.port,
                            &mut self.timeout_ms,
                            &mut actions,
                            &self.host,
                        ) || submit
                        {
                            actions.push(AppAction::RunTcpProbe {
                                host: self.host.clone(),
                                port: self.port,
                                timeout_ms: self.timeout_ms.clamp(100, 30000),
                            });
                        }
                    });
                }
                ui::ActionLayout::Vertical => {
                    let input = ui.add_sized(
                        [ui.available_width(), ui::CONTROL_HEIGHT],
                        ui::text_input(&mut self.host, "主机名或 IP 地址 (如 www.bing.com)"),
                    );
                    let submit =
                        input.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    ui.add_space(ui::SPACE_8);
                    ui.horizontal_wrapped(|ui| {
                        if tcp_controls(
                            ui,
                            &mut self.port,
                            &mut self.timeout_ms,
                            &mut actions,
                            &self.host,
                        ) || submit
                        {
                            actions.push(AppAction::RunTcpProbe {
                                host: self.host.clone(),
                                port: self.port,
                                timeout_ms: self.timeout_ms.clamp(100, 30000),
                            });
                        }
                    });
                }
            }
        });
        ui.add_space(ui::SPACE_16);
        match &self.result {
            Some(Ok(result)) => {
                let palette = ui::palette_for_ui(ui);
                ui::card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("测试结果").strong().size(15.0));
                        ui.add_space(ui::SPACE_8);
                        if result.success {
                            ui::badge(
                                ui,
                                "TCP 握手成功",
                                palette.success_text,
                                palette
                                    .success_text
                                    .gamma_multiply(if ui.visuals().dark_mode {
                                        0.22
                                    } else {
                                        0.12
                                    }),
                            );
                        } else {
                            ui::badge(
                                ui,
                                "TCP 连接失败",
                                palette.danger_text,
                                palette
                                    .danger_text
                                    .gamma_multiply(if ui.visuals().dark_mode {
                                        0.22
                                    } else {
                                        0.12
                                    }),
                            );
                        }
                    });
                    ui.add_space(ui::SPACE_12);
                    for attempt in &result.attempts {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&attempt.address).monospace());
                            ui.colored_label(palette.weak, "➔");
                            ui.label(
                                RichText::new(format!("{:.1} ms", attempt.elapsed_ms))
                                    .strong()
                                    .color(palette.accent),
                            );
                            ui.label(RichText::new(&attempt.status).color(if result.success {
                                palette.success_text
                            } else {
                                palette.danger_text
                            }));
                        });
                        ui.add_space(1.0);
                    }
                });
            }
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
    _actions: &mut Vec<AppAction>,
    _host: &str,
) -> bool {
    ui.add(egui::DragValue::new(port).range(1..=65535).prefix("端口: "));
    ui.add(
        egui::DragValue::new(timeout_ms)
            .range(100..=30000)
            .prefix("超时: ")
            .suffix(" ms"),
    );
    ui::primary_button(ui, "发起测试").clicked()
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
