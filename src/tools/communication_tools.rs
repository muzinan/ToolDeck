//! TCP、UDP 与串口通信调试工具页面。
//! 页面只维护输入、显示和定时调度状态；持续 I/O 由应用级 CommunicationDispatcher 专用会话线程承担。

use std::{
    collections::HashSet,
    net::SocketAddr,
    time::{Duration, Instant},
};

use eframe::egui::{self, RichText};

use crate::{
    core::{actions::AppAction, invocation::ToolPayload, worker::TaskResult},
    model::{
        CommunicationCommand, CommunicationConfig, CommunicationDirection, CommunicationEvent,
        CommunicationEventEnvelope, CommunicationIpFamily, CommunicationKind, CommunicationLog,
        CommunicationPeer, CommunicationRecord, CommunicationSendTarget, CommunicationSessionState,
        PayloadFormat, SerialDataBits, SerialDebugConfig, SerialFlowControl, SerialParity,
        SerialPortDescriptor, SerialStopBits, TcpDebugConfig, TcpDebugMode, UdpDebugConfig,
        UdpMulticastConfig, decode_payload, render_payload,
    },
    platform::windows::local_time_hms_millis,
    tools::{
        ToolModule, ToolUiContext,
        registry::{ToolCategory, ToolDescriptor, ToolIcon},
    },
    ui,
};

const MIN_LOG_HEIGHT: f32 = 420.0;
const MAX_LOG_HEIGHT: f32 = 950.0;
const LOG_HEIGHT_RATIO: f32 = 0.58;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum DirectionFilter {
    #[default]
    All,
    Incoming,
    Outgoing,
}

impl DirectionFilter {
    const ALL: [Self; 3] = [Self::All, Self::Incoming, Self::Outgoing];

    fn label(self) -> &'static str {
        match self {
            Self::All => "全部方向",
            Self::Incoming => "仅接收 RX",
            Self::Outgoing => "仅发送 TX",
        }
    }

    fn accepts(self, direction: CommunicationDirection) -> bool {
        match self {
            Self::All => true,
            Self::Incoming => direction == CommunicationDirection::Incoming,
            Self::Outgoing => direction == CommunicationDirection::Outgoing,
        }
    }
}

struct CommonCommunicationState {
    send_input: String,
    send_format: PayloadFormat,
    receive_format: PayloadFormat,
    append_crlf: bool,
    interval_ms: u64,
    periodic: bool,
    next_send: Option<Instant>,
    direction_filter: DirectionFilter,
    auto_scroll: bool,
    log: CommunicationLog,
    state: CommunicationSessionState,
    status_detail: String,
    error: Option<String>,
    generation: u64,
    peers: Vec<CommunicationPeer>,
}

impl Default for CommonCommunicationState {
    fn default() -> Self {
        Self {
            send_input: String::new(),
            send_format: PayloadFormat::Text,
            receive_format: PayloadFormat::Text,
            append_crlf: false,
            interval_ms: 1_000,
            periodic: false,
            next_send: None,
            direction_filter: DirectionFilter::All,
            auto_scroll: true,
            log: CommunicationLog::default(),
            state: CommunicationSessionState::Stopped,
            status_detail: "会话尚未启动".into(),
            error: None,
            generation: 0,
            peers: Vec::new(),
        }
    }
}

impl CommonCommunicationState {
    fn begin_start(&mut self) {
        self.state = CommunicationSessionState::Starting;
        self.status_detail = "正在创建通信会话".into();
        self.error = None;
        self.stop_periodic();
        self.peers.clear();
    }

    fn is_active(&self) -> bool {
        matches!(
            self.state,
            CommunicationSessionState::Connected | CommunicationSessionState::Listening
        )
    }

    fn stop_periodic(&mut self) {
        self.periodic = false;
        self.next_send = None;
    }

    fn handle_event(&mut self, envelope: CommunicationEventEnvelope) {
        if envelope.generation < self.generation {
            return;
        }
        if envelope.generation > self.generation {
            self.generation = envelope.generation;
            self.peers.clear();
        }
        match envelope.event {
            CommunicationEvent::Status { state, detail } => {
                self.state = state;
                self.status_detail.clone_from(&detail);
                self.log.push_batch(vec![CommunicationRecord {
                    timestamp: local_time_hms_millis(),
                    direction: CommunicationDirection::Status,
                    endpoint: envelope.kind.tool_id().into(),
                    payload: Vec::new(),
                    note: Some(detail.clone()),
                }]);
                if matches!(
                    state,
                    CommunicationSessionState::Failed | CommunicationSessionState::Stopped
                ) {
                    self.stop_periodic();
                }
                if state == CommunicationSessionState::Failed {
                    self.error = Some(detail);
                }
            }
            CommunicationEvent::Records(records) => self.log.push_batch(records),
            CommunicationEvent::Peers(peers) => {
                self.peers = peers;
            }
            CommunicationEvent::QueueDropped(count) => self.log.add_queue_dropped(count),
        }
    }

    fn render_status(&self, ui: &mut egui::Ui, protocol: &str, endpoint: &str) {
        let palette = ui::palette_for_ui(ui);
        let state_label = match self.state {
            CommunicationSessionState::Starting => "启动中",
            CommunicationSessionState::Listening => "监听中",
            CommunicationSessionState::Connected => "已连接",
            CommunicationSessionState::Stopped => "未连接",
            CommunicationSessionState::Failed => "失败",
        };
        let state_color = match self.state {
            CommunicationSessionState::Connected | CommunicationSessionState::Listening => {
                palette.success_text
            }
            CommunicationSessionState::Failed => palette.danger_text,
            CommunicationSessionState::Starting => palette.accent,
            CommunicationSessionState::Stopped => palette.weak,
        };
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0_f32, palette.border_subtle))
            .corner_radius(egui::CornerRadius::same(5))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui::status_pill(ui, state_label, state_color);
                    ui.separator();
                    ui.label(RichText::new(protocol).strong().color(palette.accent));
                    ui.label(RichText::new(endpoint).monospace().color(palette.text));
                    ui.separator();
                    ui.label(format!("记录：{}", self.log.records().len()));
                    ui.label(format!("载荷：{} B", self.log.payload_bytes()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(&self.status_detail)
                                .size(12.0)
                                .color(palette.weak),
                        );
                    });
                });
            });
        if let Some(error) = &self.error {
            ui.add_space(ui::SPACE_8);
            ui::state_card(ui, "通信操作失败", error, palette.danger_text);
        }
    }

    fn render_log(&mut self, ui: &mut egui::Ui, kind: CommunicationKind) -> Vec<AppAction> {
        let mut actions = Vec::new();
        let palette = ui::palette_for_ui(ui);
        let height = communication_log_height(ui.ctx().screen_rect().height());
        ui::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new("通信记录")
                        .strong()
                        .size(14.5)
                        .color(palette.text),
                );
                egui::ComboBox::from_id_salt((kind.tool_id(), "direction-filter"))
                    .selected_text(self.direction_filter.label())
                    .show_ui(ui, |ui| {
                        for filter in DirectionFilter::ALL {
                            ui.selectable_value(&mut self.direction_filter, filter, filter.label());
                        }
                    });
                egui::ComboBox::from_id_salt((kind.tool_id(), "receive-format"))
                    .selected_text(self.receive_format.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.receive_format,
                            PayloadFormat::Text,
                            PayloadFormat::Text.label(),
                        );
                        ui.selectable_value(
                            &mut self.receive_format,
                            PayloadFormat::Hex,
                            PayloadFormat::Hex.label(),
                        );
                    });
                ui.checkbox(&mut self.auto_scroll, "自动滚动");
                if ui::icon_button(ui, ui::AppIcon::Clear, "清空通信记录", false).clicked() {
                    self.log.clear();
                }
                if ui::icon_button(ui, ui::AppIcon::Copy, "复制通信记录", false).clicked() {
                    actions.push(AppAction::CopyText(self.log.export(self.receive_format)));
                }
                if ui::icon_button(ui, ui::AppIcon::Export, "导出日志", false).clicked() {
                    actions.push(AppAction::ExportCommunicationLog {
                        content: self.log.export(self.receive_format),
                        file_name: format!("{}.log", kind.tool_id()),
                    });
                }
            });
            ui.add_space(ui::SPACE_4);
            ui.label(
                RichText::new(format!(
                    "上限淘汰 {} 条 · 队列拥塞丢弃 {} 条",
                    self.log.evicted_records(),
                    self.log.queue_dropped_records()
                ))
                .size(12.0)
                .color(palette.weak),
            );
            ui.add_space(ui::SPACE_8);
            egui::ScrollArea::vertical()
                .id_salt((kind.tool_id(), "communication-log"))
                .max_height(height)
                .min_scrolled_height(height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_min_height(height);
                    for record in self
                        .log
                        .records()
                        .iter()
                        .filter(|record| self.direction_filter.accepts(record.direction))
                    {
                        render_record(ui, record, self.receive_format);
                    }
                    if self.auto_scroll {
                        ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                    }
                });
        });
        actions
    }

    fn render_send(
        &mut self,
        ui: &mut egui::Ui,
        kind: CommunicationKind,
        target: CommunicationSendTarget,
    ) -> Vec<AppAction> {
        let mut actions = Vec::new();
        let palette = ui::palette_for_ui(ui);
        ui::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("发送区").strong().color(palette.text));
                egui::ComboBox::from_id_salt((kind.tool_id(), "send-format"))
                    .selected_text(self.send_format.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut self.send_format,
                            PayloadFormat::Text,
                            PayloadFormat::Text.label(),
                        );
                        ui.selectable_value(
                            &mut self.send_format,
                            PayloadFormat::Hex,
                            PayloadFormat::Hex.label(),
                        );
                    });
                ui.checkbox(&mut self.append_crlf, "追加 CRLF");
            });
            ui.add_space(ui::SPACE_8);
            ui.add_sized(
                [ui.available_width(), 90.0],
                egui::TextEdit::multiline(&mut self.send_input)
                    .hint_text("输入要发送的文本，或空白分隔的两位 HEX 字节")
                    .font(egui::TextStyle::Monospace),
            );
            ui.add_space(ui::SPACE_8);
            ui.horizontal_wrapped(|ui| {
                if ui::primary_button(ui, "发送").clicked()
                    && let Some(action) = self.build_send_action(kind, target.clone())
                {
                    actions.push(action);
                }
                ui.add(
                    egui::DragValue::new(&mut self.interval_ms)
                        .range(10..=3_600_000)
                        .prefix("周期: ")
                        .suffix(" ms"),
                );
                let periodic_label = if self.periodic {
                    "停止定时发送"
                } else {
                    "开始定时发送"
                };
                if ui::secondary_button(ui, periodic_label).clicked() {
                    if self.periodic {
                        self.stop_periodic();
                    } else if !self.is_active() {
                        self.error = Some("请先启动并建立通信会话。".into());
                    } else if !(10..=3_600_000).contains(&self.interval_ms) {
                        self.error = Some("定时发送周期必须在 10–3600000 ms 之间。".into());
                    } else if let Some(action) = self.build_send_action(kind, target) {
                        self.periodic = true;
                        self.next_send = Some(next_timer_due(Instant::now(), self.interval_ms));
                        actions.push(action);
                    }
                }
            });
        });
        actions
    }

    fn build_send_action(
        &mut self,
        kind: CommunicationKind,
        target: CommunicationSendTarget,
    ) -> Option<AppAction> {
        if !self.is_active() {
            self.error = Some("请先启动并建立通信会话。".into());
            return None;
        }
        match decode_payload(&self.send_input, self.send_format, self.append_crlf) {
            Ok(payload) if payload.is_empty() => {
                self.error = Some("发送内容不能为空。".into());
                None
            }
            Ok(payload) => {
                self.error = None;
                Some(AppAction::SendCommunication {
                    kind,
                    command: CommunicationCommand::Send { payload, target },
                })
            }
            Err(error) => {
                self.error = Some(error);
                None
            }
        }
    }

    fn poll_periodic(
        &mut self,
        now: Instant,
        kind: CommunicationKind,
        target: CommunicationSendTarget,
    ) -> Vec<AppAction> {
        if !self.periodic || !self.is_active() {
            return Vec::new();
        }
        let Some(due) = self.next_send else {
            self.next_send = Some(next_timer_due(now, self.interval_ms));
            return Vec::new();
        };
        if now < due {
            return Vec::new();
        }
        self.next_send = Some(next_timer_due(now, self.interval_ms));
        self.build_send_action(kind, target).into_iter().collect()
    }
}

pub struct TcpDebugTool {
    mode: TcpDebugMode,
    family: CommunicationIpFamily,
    address: String,
    port: u16,
    timeout_ms: u32,
    send_all_clients: bool,
    selected_client: Option<u64>,
    common: CommonCommunicationState,
}

impl Default for TcpDebugTool {
    fn default() -> Self {
        Self {
            mode: TcpDebugMode::Client,
            family: CommunicationIpFamily::V4,
            address: "127.0.0.1".into(),
            port: 9000,
            timeout_ms: 3_000,
            send_all_clients: true,
            selected_client: None,
            common: CommonCommunicationState::default(),
        }
    }
}

impl TcpDebugTool {
    fn config(&self) -> TcpDebugConfig {
        TcpDebugConfig {
            mode: self.mode,
            family: self.family,
            address: self.address.trim().into(),
            port: self.port,
            connect_timeout_ms: self.timeout_ms,
        }
    }

    fn send_target(&self) -> CommunicationSendTarget {
        if self.mode == TcpDebugMode::Client {
            CommunicationSendTarget::Default
        } else if self.send_all_clients {
            CommunicationSendTarget::AllTcpClients
        } else {
            self.selected_client.map_or(
                CommunicationSendTarget::Default,
                CommunicationSendTarget::TcpClient,
            )
        }
    }
}

impl ToolModule for TcpDebugTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: "tcp-debug",
            name: "TCP 调试",
            description: "TCP 客户端与多客户端服务端的持续原始字节流收发",
            category: ToolCategory::Network,
            icon: ToolIcon::TcpDebug,
            keywords: &["tcp", "socket", "client", "server", "收发", "调试"],
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _context: ToolUiContext) -> Vec<AppAction> {
        let mut actions = Vec::new();
        ui::page_heading(ui, "TCP 调试", "客户端与多客户端服务端原始字节流收发");
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("tcp-debug-mode")
                    .selected_text(self.mode.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.mode, TcpDebugMode::Client, "客户端");
                        ui.selectable_value(&mut self.mode, TcpDebugMode::Server, "服务端");
                    });
                egui::ComboBox::from_id_salt("tcp-debug-family")
                    .selected_text(self.family.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.family, CommunicationIpFamily::V4, "IPv4");
                        ui.selectable_value(&mut self.family, CommunicationIpFamily::V6, "IPv6");
                    });
                ui.add_sized(
                    [260.0, ui::CONTROL_HEIGHT],
                    ui::text_input(
                        &mut self.address,
                        if self.mode == TcpDebugMode::Client {
                            "远端主机或 IP"
                        } else {
                            "本地监听地址"
                        },
                    ),
                );
                ui.add(
                    egui::DragValue::new(&mut self.port)
                        .range(1..=u16::MAX)
                        .prefix("端口: "),
                );
                ui.add(
                    egui::DragValue::new(&mut self.timeout_ms)
                        .range(100..=30_000)
                        .prefix("超时 ")
                        .suffix(" ms"),
                );
                if ui::primary_button(ui, "连接").clicked() {
                    let config = self.config();
                    match config.validate() {
                        Ok(()) => {
                            self.common.begin_start();
                            actions.push(AppAction::StartCommunication(CommunicationConfig::Tcp(
                                config,
                            )));
                        }
                        Err(error) => self.common.error = Some(error),
                    }
                }
                if ui::secondary_button(ui, "停止").clicked() {
                    self.common.stop_periodic();
                    actions.push(AppAction::StopCommunication(CommunicationKind::Tcp));
                }
            });
            if self.mode == TcpDebugMode::Server {
                ui.add_space(ui::SPACE_8);
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut self.send_all_clients, "发送给全部客户端");
                    if !self.send_all_clients {
                        let selected = self
                            .common
                            .peers
                            .iter()
                            .find(|peer| Some(peer.id) == self.selected_client)
                            .map_or("选择客户端", |peer| peer.endpoint.as_str());
                        egui::ComboBox::from_id_salt("tcp-debug-client")
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                for peer in &self.common.peers {
                                    ui.selectable_value(
                                        &mut self.selected_client,
                                        Some(peer.id),
                                        &peer.endpoint,
                                    );
                                }
                            });
                    }
                    ui.label(format!("已连接 {} / 32", self.common.peers.len()));
                });
            }
        });
        ui.add_space(ui::SPACE_12);
        let endpoint = format!("{}:{}", self.address.trim(), self.port);
        self.common.render_status(ui, "TCP", &endpoint);
        ui.add_space(ui::SPACE_12);
        actions.extend(self.common.render_log(ui, CommunicationKind::Tcp));
        ui.add_space(ui::SPACE_12);
        let send_target = self.send_target();
        actions.extend(
            self.common
                .render_send(ui, CommunicationKind::Tcp, send_target),
        );
        actions
    }

    fn handle_invocation(&mut self, _payload: ToolPayload) -> Vec<AppAction> {
        Vec::new()
    }

    fn handle_task_result(&mut self, _result: TaskResult) {}

    fn handle_communication_event(&mut self, event: CommunicationEventEnvelope) {
        self.common.handle_event(event);
        if self
            .selected_client
            .is_some_and(|id| !self.common.peers.iter().any(|peer| peer.id == id))
        {
            self.selected_client = None;
        }
    }

    fn set_busy(&mut self, _busy: bool) {}

    fn poll_actions(&mut self, now: Instant) -> Vec<AppAction> {
        let send_target = self.send_target();
        self.common
            .poll_periodic(now, CommunicationKind::Tcp, send_target)
    }
}

pub struct UdpDebugTool {
    family: CommunicationIpFamily,
    local_address: String,
    local_port: u16,
    remote_address: String,
    remote_port: u16,
    broadcast: bool,
    multicast_enabled: bool,
    multicast_group: String,
    multicast_interface: String,
    multicast_ttl: u32,
    multicast_loopback: bool,
    reply_selected_source: bool,
    selected_source: Option<SocketAddr>,
    sources: Vec<SocketAddr>,
    common: CommonCommunicationState,
}

impl Default for UdpDebugTool {
    fn default() -> Self {
        Self {
            family: CommunicationIpFamily::V4,
            local_address: "0.0.0.0".into(),
            local_port: 9000,
            remote_address: String::new(),
            remote_port: 0,
            broadcast: false,
            multicast_enabled: false,
            multicast_group: "239.255.0.1".into(),
            multicast_interface: "0.0.0.0".into(),
            multicast_ttl: 1,
            multicast_loopback: true,
            reply_selected_source: false,
            selected_source: None,
            sources: Vec::new(),
            common: CommonCommunicationState::default(),
        }
    }
}

impl UdpDebugTool {
    fn config(&self) -> UdpDebugConfig {
        UdpDebugConfig {
            family: self.family,
            local_address: self.local_address.trim().into(),
            local_port: self.local_port,
            remote_address: self.remote_address.trim().into(),
            remote_port: self.remote_port,
            broadcast: self.broadcast,
            multicast: self.multicast_enabled.then(|| UdpMulticastConfig {
                group: self.multicast_group.trim().into(),
                interface: self.multicast_interface.trim().into(),
                ttl: self.multicast_ttl,
                loopback: self.multicast_loopback,
            }),
        }
    }

    fn send_target(&self) -> CommunicationSendTarget {
        if self.reply_selected_source {
            self.selected_source.map_or(
                CommunicationSendTarget::Default,
                CommunicationSendTarget::UdpSource,
            )
        } else {
            CommunicationSendTarget::Default
        }
    }
}

impl ToolModule for UdpDebugTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: "udp-debug",
            name: "UDP 调试",
            description: "UDP 单播、IPv4 广播与单组 IPv4 组播数据报调试",
            category: ToolCategory::Network,
            icon: ToolIcon::UdpDebug,
            keywords: &["udp", "datagram", "broadcast", "multicast", "组播", "广播"],
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _context: ToolUiContext) -> Vec<AppAction> {
        let mut actions = Vec::new();
        ui::page_heading(ui, "UDP 调试", "保留数据报边界、来源与目标信息");
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                egui::ComboBox::from_id_salt("udp-debug-family")
                    .selected_text(self.family.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.family, CommunicationIpFamily::V4, "IPv4");
                        ui.selectable_value(&mut self.family, CommunicationIpFamily::V6, "IPv6");
                    });
                ui.label("本地绑定");
                ui.add_sized(
                    [170.0, ui::CONTROL_HEIGHT],
                    ui::text_input(&mut self.local_address, "本地地址"),
                );
                ui.add(egui::DragValue::new(&mut self.local_port).prefix("端口: "));
                ui.label("默认远端");
                ui.add_sized(
                    [190.0, ui::CONTROL_HEIGHT],
                    ui::text_input(&mut self.remote_address, "可留空"),
                );
                ui.add(egui::DragValue::new(&mut self.remote_port).prefix("端口: "));
                if ui::primary_button(ui, "启动").clicked() {
                    let config = self.config();
                    match config.validate() {
                        Ok(()) => {
                            self.common.begin_start();
                            self.sources.clear();
                            self.selected_source = None;
                            actions.push(AppAction::StartCommunication(CommunicationConfig::Udp(
                                config,
                            )));
                        }
                        Err(error) => self.common.error = Some(error),
                    }
                }
                if ui::secondary_button(ui, "停止").clicked() {
                    self.common.stop_periodic();
                    actions.push(AppAction::StopCommunication(CommunicationKind::Udp));
                }
            });
            ui.add_space(ui::SPACE_8);
            egui::CollapsingHeader::new("广播与组播")
                .id_salt("udp-broadcast-multicast")
                .default_open(false)
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.add_enabled_ui(self.family == CommunicationIpFamily::V4, |ui| {
                            ui.checkbox(&mut self.broadcast, "IPv4 广播");
                            ui.checkbox(&mut self.multicast_enabled, "加入 IPv4 组播组");
                        });
                        if self.multicast_enabled {
                            ui.add_sized(
                                [145.0, ui::CONTROL_HEIGHT],
                                ui::text_input(&mut self.multicast_group, "组播地址"),
                            );
                            ui.add_sized(
                                [145.0, ui::CONTROL_HEIGHT],
                                ui::text_input(&mut self.multicast_interface, "本地 IPv4 接口"),
                            );
                            ui.add(
                                egui::DragValue::new(&mut self.multicast_ttl)
                                    .range(1..=255)
                                    .prefix("TTL: "),
                            );
                            ui.checkbox(&mut self.multicast_loopback, "本机回环");
                        }
                    });
                });
            ui.add_space(ui::SPACE_8);
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.reply_selected_source, "回复选中接收来源");
                if self.reply_selected_source {
                    let selected = self
                        .selected_source
                        .map_or_else(|| "选择来源".into(), |source| source.to_string());
                    egui::ComboBox::from_id_salt("udp-debug-source")
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            for source in &self.sources {
                                ui.selectable_value(
                                    &mut self.selected_source,
                                    Some(*source),
                                    source.to_string(),
                                );
                            }
                        });
                }
            });
        });
        ui.add_space(ui::SPACE_12);
        let endpoint = format!("{}:{}", self.local_address.trim(), self.local_port);
        self.common.render_status(ui, "UDP", &endpoint);
        ui.add_space(ui::SPACE_12);
        actions.extend(self.common.render_log(ui, CommunicationKind::Udp));
        ui.add_space(ui::SPACE_12);
        let send_target = self.send_target();
        actions.extend(
            self.common
                .render_send(ui, CommunicationKind::Udp, send_target),
        );
        actions
    }

    fn handle_invocation(&mut self, _payload: ToolPayload) -> Vec<AppAction> {
        Vec::new()
    }

    fn handle_task_result(&mut self, _result: TaskResult) {}

    fn handle_communication_event(&mut self, event: CommunicationEventEnvelope) {
        if let CommunicationEvent::Records(records) = &event.event {
            let mut known = self.sources.iter().copied().collect::<HashSet<_>>();
            for source in records
                .iter()
                .filter(|record| record.direction == CommunicationDirection::Incoming)
                .filter_map(|record| record.endpoint.parse::<SocketAddr>().ok())
            {
                if known.insert(source) {
                    self.sources.push(source);
                }
                self.selected_source.get_or_insert(source);
            }
        }
        self.common.handle_event(event);
    }

    fn set_busy(&mut self, _busy: bool) {}

    fn poll_actions(&mut self, now: Instant) -> Vec<AppAction> {
        let send_target = self.send_target();
        self.common
            .poll_periodic(now, CommunicationKind::Udp, send_target)
    }
}

pub struct SerialDebugTool {
    ports: Vec<SerialPortDescriptor>,
    selected_port: String,
    baud_rate: u32,
    data_bits: SerialDataBits,
    parity: SerialParity,
    stop_bits: SerialStopBits,
    flow_control: SerialFlowControl,
    read_timeout_ms: u64,
    refresh_requested: bool,
    refresh_busy: bool,
    common: CommonCommunicationState,
}

impl Default for SerialDebugTool {
    fn default() -> Self {
        Self {
            ports: Vec::new(),
            selected_port: String::new(),
            baud_rate: 115_200,
            data_bits: SerialDataBits::Eight,
            parity: SerialParity::None,
            stop_bits: SerialStopBits::One,
            flow_control: SerialFlowControl::None,
            read_timeout_ms: 50,
            refresh_requested: true,
            refresh_busy: false,
            common: CommonCommunicationState::default(),
        }
    }
}

impl SerialDebugTool {
    fn config(&self) -> SerialDebugConfig {
        SerialDebugConfig {
            port_name: self.selected_port.clone(),
            baud_rate: self.baud_rate,
            data_bits: self.data_bits,
            parity: self.parity,
            stop_bits: self.stop_bits,
            flow_control: self.flow_control,
            read_timeout_ms: self.read_timeout_ms,
        }
    }
}

impl ToolModule for SerialDebugTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: "serial-debug",
            name: "串口调试",
            description: "独占打开 COM 口并持续进行文本或 HEX 收发",
            category: ToolCategory::Developer,
            icon: ToolIcon::SerialDebug,
            keywords: &["serial", "com", "uart", "串口", "波特率", "调试"],
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _context: ToolUiContext) -> Vec<AppAction> {
        let mut actions = Vec::new();
        ui::page_heading(ui, "串口调试", "配置 COM 口并持续收发文本或 HEX 数据");
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                let selected = self
                    .ports
                    .iter()
                    .find(|port| port.port_name == self.selected_port)
                    .map_or_else(
                        || {
                            if self.selected_port.is_empty() {
                                "选择 COM 口".into()
                            } else {
                                self.selected_port.clone()
                            }
                        },
                        SerialPortDescriptor::display_name,
                    );
                egui::ComboBox::from_id_salt("serial-debug-port")
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        for port in &self.ports {
                            ui.selectable_value(
                                &mut self.selected_port,
                                port.port_name.clone(),
                                port.display_name(),
                            )
                            .on_hover_text(serial_port_detail(port));
                        }
                    });
                if ui::secondary_button(
                    ui,
                    if self.refresh_busy {
                        "刷新中..."
                    } else {
                        "刷新串口"
                    },
                )
                .clicked()
                {
                    self.refresh_requested = true;
                }
                ui.add(
                    egui::DragValue::new(&mut self.baud_rate)
                        .range(1..=4_000_000)
                        .prefix("波特率: "),
                );
                egui::ComboBox::from_id_salt("serial-data-bits")
                    .selected_text(format!("{} 数据位", self.data_bits.label()))
                    .show_ui(ui, |ui| {
                        for value in SerialDataBits::ALL {
                            ui.selectable_value(&mut self.data_bits, value, value.label());
                        }
                    });
                egui::ComboBox::from_id_salt("serial-parity")
                    .selected_text(self.parity.label())
                    .show_ui(ui, |ui| {
                        for value in SerialParity::ALL {
                            ui.selectable_value(&mut self.parity, value, value.label());
                        }
                    });
                egui::ComboBox::from_id_salt("serial-stop-bits")
                    .selected_text(format!("{} 停止位", self.stop_bits.label()))
                    .show_ui(ui, |ui| {
                        for value in SerialStopBits::ALL {
                            ui.selectable_value(&mut self.stop_bits, value, value.label());
                        }
                    });
                egui::ComboBox::from_id_salt("serial-flow-control")
                    .selected_text(format!("{}流控", self.flow_control.label()))
                    .show_ui(ui, |ui| {
                        for value in SerialFlowControl::ALL {
                            ui.selectable_value(&mut self.flow_control, value, value.label());
                        }
                    });
                ui.add(
                    egui::DragValue::new(&mut self.read_timeout_ms)
                        .range(10..=1_000)
                        .prefix("超时 ")
                        .suffix(" ms"),
                );
                if ui::primary_button(ui, "打开").clicked() {
                    let config = self.config();
                    match config.validate() {
                        Ok(()) => {
                            self.common.begin_start();
                            actions.push(AppAction::StartCommunication(
                                CommunicationConfig::Serial(config),
                            ));
                        }
                        Err(error) => self.common.error = Some(error),
                    }
                }
                if ui::secondary_button(ui, "关闭").clicked() {
                    self.common.stop_periodic();
                    actions.push(AppAction::StopCommunication(CommunicationKind::Serial));
                }
            });
        });
        ui.add_space(ui::SPACE_12);
        let endpoint = if self.selected_port.is_empty() {
            "未选择".to_owned()
        } else {
            self.selected_port.clone()
        };
        self.common.render_status(ui, "SERIAL", &endpoint);
        ui.add_space(ui::SPACE_12);
        actions.extend(self.common.render_log(ui, CommunicationKind::Serial));
        ui.add_space(ui::SPACE_12);
        actions.extend(self.common.render_send(
            ui,
            CommunicationKind::Serial,
            CommunicationSendTarget::Default,
        ));
        actions
    }

    fn handle_invocation(&mut self, _payload: ToolPayload) -> Vec<AppAction> {
        Vec::new()
    }

    fn handle_task_result(&mut self, result: TaskResult) {
        if let TaskResult::SerialPorts(result) = result {
            self.refresh_busy = false;
            match result {
                Ok(ports) => {
                    self.ports = ports;
                    if !self
                        .ports
                        .iter()
                        .any(|port| port.port_name == self.selected_port)
                    {
                        self.selected_port = self
                            .ports
                            .first()
                            .map_or_else(String::new, |port| port.port_name.clone());
                    }
                }
                Err(error) => {
                    let (_, detail) = error.user_message();
                    self.common.error = Some(detail);
                }
            }
        }
    }

    fn handle_communication_event(&mut self, event: CommunicationEventEnvelope) {
        self.common.handle_event(event);
    }

    fn set_busy(&mut self, busy: bool) {
        self.refresh_busy = busy;
    }

    fn poll_actions(&mut self, now: Instant) -> Vec<AppAction> {
        let mut actions = self.common.poll_periodic(
            now,
            CommunicationKind::Serial,
            CommunicationSendTarget::Default,
        );
        if self.refresh_requested && !self.refresh_busy {
            self.refresh_requested = false;
            self.refresh_busy = true;
            actions.push(AppAction::RefreshSerialPorts);
        }
        actions
    }
}

fn communication_log_height(viewport_height: f32) -> f32 {
    (viewport_height * LOG_HEIGHT_RATIO).clamp(MIN_LOG_HEIGHT, MAX_LOG_HEIGHT)
}

fn render_record(ui: &mut egui::Ui, record: &CommunicationRecord, format: PayloadFormat) {
    let palette = ui::palette_for_ui(ui);
    let direction_color = match record.direction {
        CommunicationDirection::Incoming => palette.success_text,
        CommunicationDirection::Outgoing => palette.accent,
        CommunicationDirection::Status => palette.warning_text,
    };
    let content = record
        .note
        .clone()
        .unwrap_or_else(|| render_payload(&record.payload, format));
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new(&record.timestamp)
                .monospace()
                .size(12.0)
                .color(palette.weak),
        );
        ui::badge(
            ui,
            record.direction.label(),
            direction_color,
            direction_color.gamma_multiply(if ui.visuals().dark_mode { 0.18 } else { 0.10 }),
        );
        ui.label(
            RichText::new(&record.endpoint)
                .monospace()
                .size(12.0)
                .color(palette.text),
        );
        ui.label(
            RichText::new(format!("{} B", record.payload.len()))
                .size(12.0)
                .color(palette.weak),
        );
        ui.add(
            egui::Label::new(
                RichText::new(content)
                    .monospace()
                    .size(12.5)
                    .color(palette.text),
            )
            .wrap(),
        );
    });
    ui.add_space(2.0);
}

fn serial_port_detail(port: &SerialPortDescriptor) -> String {
    let mut parts = vec![format!("类型：{}", port.port_type)];
    if let Some(manufacturer) = &port.manufacturer {
        parts.push(format!("厂商：{manufacturer}"));
    }
    if let (Some(vid), Some(pid)) = (port.vid, port.pid) {
        parts.push(format!("VID:PID {vid:04X}:{pid:04X}"));
    }
    if let Some(serial_number) = &port.serial_number {
        parts.push(format!("序列号：{serial_number}"));
    }
    parts.join("\n")
}

fn next_timer_due(now: Instant, interval_ms: u64) -> Instant {
    now + Duration::from_millis(interval_ms.clamp(10, 3_600_000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_area_uses_viewport_ratio_with_bounds() {
        assert_eq!(communication_log_height(500.0), MIN_LOG_HEIGHT);
        assert_eq!(communication_log_height(1_000.0), 580.0);
        assert_eq!(communication_log_height(2_000.0), MAX_LOG_HEIGHT);
    }

    #[test]
    fn periodic_deadline_clamps_to_supported_boundaries() {
        let now = Instant::now();
        assert_eq!(
            next_timer_due(now, 1).duration_since(now),
            Duration::from_millis(10)
        );
        assert_eq!(
            next_timer_due(now, 9_000_000).duration_since(now),
            Duration::from_millis(3_600_000)
        );
    }
}
