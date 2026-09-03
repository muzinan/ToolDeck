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
        ToolModule, ToolUiContext, UiReviewVariant,
        registry::{ToolCategory, ToolDescriptor, ToolIcon},
    },
    ui,
};

const MAX_LOG_HEIGHT: f32 = 380.0;
const LOG_HEIGHT_RATIO: f32 = 0.29;
const COMPACT_LOG_HEIGHT: f32 = 132.0;
const DESIGN_VIEWPORT_HEIGHT: f32 = 960.0;
const RECORD_HEADER_HEIGHT: f32 = 34.0;
const RECORD_ROW_HEIGHT: f32 = 38.0;
const SERVER_RECORD_HEADER_HEIGHT: f32 = 26.0;
const SERVER_RECORD_ROW_HEIGHT: f32 = 22.0;

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
            Self::All => "全部",
            Self::Incoming => "接收",
            Self::Outgoing => "发送",
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
    log_search: String,
    auto_scroll: bool,
    log: CommunicationLog,
    state: CommunicationSessionState,
    session_started_at: Option<Instant>,
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
            log_search: String::new(),
            auto_scroll: true,
            log: CommunicationLog::default(),
            state: CommunicationSessionState::Stopped,
            session_started_at: None,
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
        self.session_started_at = None;
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

    fn render_inline_state(&self, ui: &mut egui::Ui) {
        let palette = ui::palette_for_ui(ui);
        let (label, color) = match self.state {
            CommunicationSessionState::Starting => ("启动中", palette.accent),
            CommunicationSessionState::Listening => ("监听中", palette.success_text),
            CommunicationSessionState::Connected => ("已连接", palette.success_text),
            CommunicationSessionState::Stopped => ("未连接", palette.weak),
            CommunicationSessionState::Failed => ("失败", palette.danger_text),
        };
        ui::status_pill(ui, label, color);
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
                if matches!(
                    state,
                    CommunicationSessionState::Connected | CommunicationSessionState::Listening
                ) && !matches!(
                    self.state,
                    CommunicationSessionState::Connected | CommunicationSessionState::Listening
                ) {
                    self.session_started_at = Some(Instant::now());
                } else if matches!(
                    state,
                    CommunicationSessionState::Failed | CommunicationSessionState::Stopped
                ) {
                    self.session_started_at = None;
                }
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

    fn render_status(
        &self,
        ui: &mut egui::Ui,
        protocol: &str,
        endpoint: &str,
        show_duration: bool,
    ) {
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
        ui.horizontal_wrapped(|ui| {
            ui::status_pill(ui, state_label, state_color);
            ui.separator();
            ui.label(RichText::new(protocol).strong().color(palette.accent));
            ui.label(RichText::new(endpoint).monospace().color(palette.text));
            ui.separator();
            let (received, sent) = self.byte_totals();
            ui.label(format!("已发送 {}", format_bytes(sent)));
            ui.separator();
            ui.label(format!("已接收 {}", format_bytes(received)));
            ui.separator();
            if show_duration && let Some(started_at) = self.session_started_at {
                ui.label(format!("连接 {}", format_elapsed(started_at.elapsed())));
                ui.separator();
            }
            ui.label(format!("队列丢弃 {}", self.log.queue_dropped_records()));
        });
    }

    fn render_error(&self, ui: &mut egui::Ui) {
        let palette = ui::palette_for_ui(ui);
        if let Some(error) = &self.error {
            ui.add_space(19.0);
            ui::state_card(ui, "通信操作失败", error, palette.danger_text);
        }
    }

    fn render_log(&mut self, ui: &mut egui::Ui, kind: CommunicationKind) -> Vec<AppAction> {
        let mut actions = Vec::new();
        let palette = ui::palette_for_ui(ui);
        let server_log = kind == CommunicationKind::Tcp && !self.peers.is_empty();
        let height = if server_log {
            communication_log_height(ui.ctx().screen_rect().height()).max(360.0)
        } else {
            communication_log_height(ui.ctx().screen_rect().height())
        };
        let header_height = if server_log {
            SERVER_RECORD_HEADER_HEIGHT
        } else {
            RECORD_HEADER_HEIGHT
        };
        let row_height = if server_log {
            SERVER_RECORD_ROW_HEIGHT
        } else {
            RECORD_ROW_HEIGHT
        };
        ui::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("数据日志")
                        .strong()
                        .size(14.5)
                        .color(palette.text),
                );
                if kind == CommunicationKind::Udp {
                    let (incoming, outgoing) = self.packet_totals();
                    let (received, sent) = self.byte_totals();
                    ui.label(format!("数据包：{}", incoming + outgoing));
                    ui.label(
                        RichText::new(format!("接收：{incoming}（{}）", format_bytes(received)))
                            .color(palette.receive_text),
                    );
                    ui.label(
                        RichText::new(format!("发送：{outgoing}（{}）", format_bytes(sent)))
                            .color(palette.success_text),
                    );
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if log_toolbar_action(ui, ui::AppIcon::Export, "导出", "导出日志") {
                        actions.push(AppAction::ExportCommunicationLog {
                            content: self.log.export(self.receive_format),
                            file_name: format!("{}.log", kind.tool_id()),
                        });
                    }
                    if log_toolbar_action(ui, ui::AppIcon::Copy, "复制", "复制通信记录") {
                        actions.push(AppAction::CopyText(self.log.export(self.receive_format)));
                    }
                    if log_toolbar_action(ui, ui::AppIcon::Clear, "清空", "清空通信记录") {
                        self.log.clear();
                    }
                    if log_toolbar_action(
                        ui,
                        ui::AppIcon::Pause,
                        "暂停自动滚动",
                        "暂停或恢复日志自动滚动",
                    ) {
                        self.auto_scroll = !self.auto_scroll;
                    }
                    ui.separator();
                    egui::ComboBox::from_id_salt((kind.tool_id(), "direction-filter"))
                        .selected_text(self.direction_filter.label())
                        .show_ui(ui, |ui| {
                            for filter in DirectionFilter::ALL {
                                ui.selectable_value(
                                    &mut self.direction_filter,
                                    filter,
                                    filter.label(),
                                );
                            }
                        });
                    ui.label("方向");
                });
            });
            ui.add_space(ui::SPACE_8);
            let search_width = if ui.available_width() < 720.0 {
                132.0
            } else {
                220.0
            };
            ui.add_sized(
                [search_width, ui::CONTROL_HEIGHT],
                ui::text_input(&mut self.log_search, "搜索日志"),
            );
            ui.add_space(ui::SPACE_8);
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                render_record_header(ui, header_height);
                egui::ScrollArea::both()
                    .id_salt((kind.tool_id(), "communication-log"))
                    .max_height(height)
                    .min_scrolled_height(height)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.set_min_height(height);
                        for record in self
                            .log
                            .records()
                            .iter()
                            .filter(|record| self.direction_filter.accepts(record.direction))
                            .filter(|record| {
                                record_matches(record, &self.log_search, self.receive_format)
                            })
                        {
                            render_record(ui, record, self.receive_format, row_height);
                        }
                        if self.auto_scroll {
                            ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                        }
                    });
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
        let use_expanded_tcp_layout = kind == CommunicationKind::Tcp;
        ui::card(ui, |ui| {
            if use_expanded_tcp_layout {
                ui.add_space(6.0);
            }
            ui.label(RichText::new("发送数据").strong().color(palette.text));
            ui.add_space(if use_expanded_tcp_layout {
                16.0
            } else {
                ui::SPACE_8
            });
            let full_width = ui.available_width();
            let compact = full_width < 720.0;
            let controls_width = if compact { full_width } else { 170.0 };
            let send_width = if compact { full_width } else { 188.0 };
            let editor_width = if compact {
                full_width
            } else {
                (full_width - controls_width - send_width - ui::SPACE_16 * 2.0).max(260.0)
            };
            let mut send_clicked = false;
            let mut periodic_clicked = false;
            let render_controls = |ui: &mut egui::Ui, state: &mut Self| {
                ui.horizontal(|ui| {
                    ui.label("接收显示");
                    format_segment(ui, &mut state.receive_format, (kind.tool_id(), "receive"));
                });
                ui.add_space(ui::SPACE_8);
                ui.horizontal(|ui| {
                    ui.label("发送模式");
                    format_segment(ui, &mut state.send_format, (kind.tool_id(), "send"));
                });
            };
            let render_editor = |ui: &mut egui::Ui, state: &mut Self| {
                ui.add_sized(
                    [ui.available_width(), 108.0],
                    egui::TextEdit::multiline(&mut state.send_input)
                        .hint_text("输入文本或空白分隔的 HEX 字节")
                        .font(egui::TextStyle::Monospace),
                );
                ui.horizontal(|ui| {
                    ui.checkbox(&mut state.append_crlf, "追加 CRLF");
                    ui.label(format!("{} 字节", state.pending_payload_len()));
                });
            };
            let render_send_actions =
                |ui: &mut egui::Ui,
                 state: &mut Self,
                 send_clicked: &mut bool,
                 periodic_clicked: &mut bool| {
                    *periodic_clicked |=
                        ui::toggle_switch(ui, &mut state.periodic, "定时发送").changed();
                    ui.horizontal(|ui| {
                        ui.label("间隔(ms)");
                        ui.add(egui::DragValue::new(&mut state.interval_ms).range(10..=3_600_000));
                    });
                    ui.add_space(ui::SPACE_8);
                    *send_clicked |= ui::success_button_sized(
                        ui,
                        "发送",
                        [ui.available_width().max(120.0), 56.0],
                    )
                    .clicked();
                };
            if compact {
                render_controls(ui, self);
                ui.add_space(ui::SPACE_8);
                render_editor(ui, self);
                ui.add_space(ui::SPACE_8);
                render_send_actions(ui, self, &mut send_clicked, &mut periodic_clicked);
            } else {
                ui.horizontal_top(|ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(controls_width, 134.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| render_controls(ui, self),
                    );
                    ui.add_space(ui::SPACE_8);
                    ui.allocate_ui_with_layout(
                        egui::vec2(editor_width, 134.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| render_editor(ui, self),
                    );
                    ui.add_space(ui::SPACE_8);
                    ui.allocate_ui_with_layout(
                        egui::vec2(send_width, 134.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            render_send_actions(ui, self, &mut send_clicked, &mut periodic_clicked)
                        },
                    );
                });
            }
            if send_clicked && let Some(action) = self.build_send_action(kind, target.clone()) {
                actions.push(action);
            }
            if periodic_clicked {
                if !self.periodic {
                    self.next_send = None;
                } else if !self.is_active() {
                    self.stop_periodic();
                    self.error = Some("请先建立通信会话。".into());
                } else if let Some(action) = self.build_send_action(kind, target) {
                    self.next_send = Some(next_timer_due(Instant::now(), self.interval_ms));
                    actions.push(action);
                } else {
                    self.stop_periodic();
                }
            }
        });
        actions
    }

    fn byte_totals(&self) -> (usize, usize) {
        self.log
            .records()
            .iter()
            .fold((0, 0), |totals, record| match record.direction {
                CommunicationDirection::Incoming => {
                    (totals.0.saturating_add(record.payload.len()), totals.1)
                }
                CommunicationDirection::Outgoing => {
                    (totals.0, totals.1.saturating_add(record.payload.len()))
                }
                CommunicationDirection::Status => totals,
            })
    }

    fn packet_totals(&self) -> (usize, usize) {
        self.log
            .records()
            .iter()
            .fold((0, 0), |totals, record| match record.direction {
                CommunicationDirection::Incoming => (totals.0.saturating_add(1), totals.1),
                CommunicationDirection::Outgoing => (totals.0, totals.1.saturating_add(1)),
                CommunicationDirection::Status => totals,
            })
    }

    fn pending_payload_len(&self) -> usize {
        decode_payload(&self.send_input, self.send_format, self.append_crlf)
            .map_or(0, |payload| payload.len())
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

fn record_matches(record: &CommunicationRecord, query: &str, format: PayloadFormat) -> bool {
    let query = query.trim().to_lowercase();
    query.is_empty()
        || record.endpoint.to_lowercase().contains(&query)
        || record
            .note
            .as_deref()
            .is_some_and(|note| note.to_lowercase().contains(&query))
        || render_payload(&record.payload, format)
            .to_lowercase()
            .contains(&query)
}

pub struct TcpDebugTool {
    mode: TcpDebugMode,
    family: CommunicationIpFamily,
    address: String,
    port: u16,
    timeout_ms: u32,
    auto_reconnect: bool,
    reconnect_interval_ms: u64,
    reconnect_armed: bool,
    next_reconnect: Option<Instant>,
    send_all_clients: bool,
    selected_client: Option<u64>,
    review_seeded: bool,
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
            auto_reconnect: false,
            reconnect_interval_ms: 3_000,
            reconnect_armed: false,
            next_reconnect: None,
            send_all_clients: true,
            selected_client: None,
            review_seeded: false,
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

    fn render_session_controls(&mut self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let active = self.common.is_active();
        let start_label = if self.mode == TcpDebugMode::Client {
            "连接"
        } else {
            "启动"
        };
        if ui
            .add_enabled_ui(!active, |ui| {
                ui::primary_button_sized(ui, start_label, [68.0, ui::CONTROL_HEIGHT])
            })
            .inner
            .clicked()
        {
            let config = self.config();
            match config.validate() {
                Ok(()) => {
                    self.common.begin_start();
                    self.reconnect_armed = self.mode == TcpDebugMode::Client && self.auto_reconnect;
                    self.next_reconnect = None;
                    actions.push(AppAction::StartCommunication(CommunicationConfig::Tcp(
                        config,
                    )));
                }
                Err(error) => self.common.error = Some(error),
            }
        }
        let stop_label = if self.mode == TcpDebugMode::Client {
            "断开"
        } else {
            "停止"
        };
        if ui
            .add_enabled_ui(active, |ui| {
                ui::danger_button_sized(ui, stop_label, [68.0, ui::CONTROL_HEIGHT])
            })
            .inner
            .clicked()
        {
            self.common.stop_periodic();
            self.reconnect_armed = false;
            self.next_reconnect = None;
            actions.push(AppAction::StopCommunication(CommunicationKind::Tcp));
        }
    }

    fn seed_review(&mut self, variant: UiReviewVariant) {
        self.review_seeded = true;
        self.mode = if variant == UiReviewVariant::TcpServer {
            TcpDebugMode::Server
        } else {
            TcpDebugMode::Client
        };
        self.common.state = if self.mode == TcpDebugMode::Server {
            CommunicationSessionState::Listening
        } else {
            CommunicationSessionState::Connected
        };
        self.common.session_started_at = Some(
            Instant::now()
                .checked_sub(Duration::from_secs(12 * 60 + 48))
                .unwrap_or_else(Instant::now),
        );
        self.common.status_detail = if self.mode == TcpDebugMode::Server {
            "正在监听 0.0.0.0:9000".into()
        } else {
            "已连接 127.0.0.1:9000".into()
        };
        self.common.send_input = "Hello World".into();
        self.common.append_crlf = true;
        self.common.auto_scroll = false;
        let (outgoing_endpoint, incoming_endpoint) = if self.mode == TcpDebugMode::Server {
            ("127.0.0.1:52314", "127.0.0.1:52314")
        } else {
            (
                "127.0.0.1:52314 → 127.0.0.1:9000",
                "127.0.0.1:9000 → 127.0.0.1:52314",
            )
        };
        let records = if self.mode == TcpDebugMode::Server {
            review_server_records(outgoing_endpoint, incoming_endpoint)
        } else {
            review_records(outgoing_endpoint, incoming_endpoint)
        };
        self.common.log.push_batch(records);
        if self.mode == TcpDebugMode::Client {
            self.auto_reconnect = true;
        } else {
            self.address = "0.0.0.0".into();
            self.common.peers = vec![
                CommunicationPeer {
                    id: 1,
                    endpoint: "127.0.0.1:52314".into(),
                    connected_at: "14:22:30.987".into(),
                    received_bytes: 156,
                    sent_bytes: 172,
                },
                CommunicationPeer {
                    id: 2,
                    endpoint: "127.0.0.1:52315".into(),
                    connected_at: "14:24:00.113".into(),
                    received_bytes: 78,
                    sent_bytes: 91,
                },
                CommunicationPeer {
                    id: 3,
                    endpoint: "127.0.0.1:52316".into(),
                    connected_at: "14:24:14.901".into(),
                    received_bytes: 62,
                    sent_bytes: 77,
                },
            ];
            self.selected_client = Some(1);
        }
    }
}

impl ToolModule for TcpDebugTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: "tcp-debug",
            name: "TCP 调试",
            description: "TCP 客户端与多客户端服务端的持续原始字节流收发",
            category: ToolCategory::Communication,
            icon: ToolIcon::TcpDebug,
            keywords: &["tcp", "socket", "client", "server", "收发", "调试"],
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, context: ToolUiContext) -> Vec<AppAction> {
        if context.review_mode && !self.review_seeded {
            self.seed_review(context.review_variant);
        }
        let mut actions = Vec::new();
        ui::page_heading(ui, "TCP 调试", "");
        ui.add_space(6.0);
        ui::card(ui, |ui| {
            let active = self.common.is_active();
            ui.spacing_mut().item_spacing.x = ui::SPACE_12;
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.label("模式");
                    ui.add_enabled_ui(!active, |ui| {
                        ui.horizontal(|ui| {
                            for mode in [TcpDebugMode::Client, TcpDebugMode::Server] {
                                ui.selectable_value(&mut self.mode, mode, mode.label());
                            }
                        });
                    });
                });
                ui.vertical(|ui| {
                    ui.label("地址族");
                    ui.add_enabled_ui(!active, |ui| {
                        ui.horizontal(|ui| {
                            for family in [CommunicationIpFamily::V4, CommunicationIpFamily::V6] {
                                ui.selectable_value(&mut self.family, family, family.label());
                            }
                        });
                    });
                });
                ui.vertical(|ui| {
                    ui.label(if self.mode == TcpDebugMode::Client {
                        "目标地址"
                    } else {
                        "监听地址"
                    });
                    ui.add_enabled_ui(!active, |ui| {
                        ui.add_sized(
                            [156.0, ui::CONTROL_HEIGHT],
                            ui::text_input(&mut self.address, "地址"),
                        );
                    });
                });
                ui.vertical(|ui| {
                    ui.label("端口");
                    ui.add_enabled_ui(!active, |ui| {
                        ui.add_sized(
                            [78.0, ui::CONTROL_HEIGHT],
                            egui::DragValue::new(&mut self.port).range(1..=u16::MAX),
                        );
                    });
                });
                if self.mode == TcpDebugMode::Client {
                    ui.vertical(|ui| {
                        ui.label("超时(ms)");
                        ui.add_enabled_ui(!active, |ui| {
                            ui.add_sized(
                                [82.0, ui::CONTROL_HEIGHT],
                                egui::DragValue::new(&mut self.timeout_ms).range(100..=30_000),
                            );
                        });
                    });
                    ui.vertical(|ui| {
                        ui.label("自动重连");
                        ui::toggle_switch(ui, &mut self.auto_reconnect, "");
                    });
                    ui.vertical(|ui| {
                        ui.label("重连间隔(ms)");
                        ui.add_enabled_ui(self.auto_reconnect, |ui| {
                            ui.add_sized(
                                [88.0, ui::CONTROL_HEIGHT],
                                egui::DragValue::new(&mut self.reconnect_interval_ms)
                                    .range(100..=60_000),
                            );
                        });
                    });
                }
                ui.vertical(|ui| {
                    ui.label("状态");
                    ui.horizontal(|ui| {
                        self.common.render_inline_state(ui);
                        self.render_session_controls(ui, &mut actions);
                    });
                });
            });
            ui.add_space(4.0);
            ui.separator();
            ui.add_space(4.0);
            if self.mode == TcpDebugMode::Client {
                let (received, sent) = self.common.byte_totals();
                ui.horizontal_wrapped(|ui| {
                    ui.label("已发送");
                    ui.label(RichText::new(format_bytes(sent)).monospace());
                    ui.separator();
                    ui.label("已接收");
                    ui.label(RichText::new(format_bytes(received)).monospace());
                    ui.separator();
                    ui.label("连接");
                    ui.label(
                        RichText::new(
                            self.common
                                .session_started_at
                                .map_or_else(|| "--:--:--".into(), |started| {
                                    format_elapsed(started.elapsed())
                                }),
                        )
                        .monospace(),
                    );
                    ui.separator();
                    ui.label("队列丢弃");
                    ui.label(
                        RichText::new(self.common.log.queue_dropped_records().to_string())
                            .monospace(),
                    );
                });
            } else {
                ui.horizontal_wrapped(|ui| {
                    ui.label("发送对象");
                    ui.radio_value(&mut self.send_all_clients, true, "全部客户端");
                    ui.radio_value(&mut self.send_all_clients, false, "指定客户端");
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
                    ui.separator();
                    self.common.render_inline_state(ui);
                    ui.label(format!("已连接 {} / 32", self.common.peers.len()));
                });
            }
        });
        self.common.render_error(ui);
        ui.add_space(ui::SPACE_12);
        if self.mode == TcpDebugMode::Server && ui.available_width() >= 900.0 {
            let available_width = ui.available_width();
            let row_height = communication_log_height(ui.ctx().screen_rect().height()).max(360.0) + 94.0;
            let gap = ui::SPACE_12;
            let content_width = available_width - gap;
            let log_width = content_width * 0.56;
            let peer_width = content_width - log_width;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                ui.allocate_ui_with_layout(
                    egui::vec2(log_width, row_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        actions.extend(self.common.render_log(ui, CommunicationKind::Tcp));
                    },
                );
                ui.allocate_ui_with_layout(
                    egui::vec2(peer_width, row_height),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        render_tcp_peer_list(ui, &self.common.peers, &mut self.selected_client);
                    },
                );
            });
        } else {
            actions.extend(self.common.render_log(ui, CommunicationKind::Tcp));
            if self.mode == TcpDebugMode::Server {
                ui.add_space(ui::SPACE_12);
                render_tcp_peer_list(ui, &self.common.peers, &mut self.selected_client);
            }
        }
        ui.add_space(6.0);
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
        if let CommunicationEvent::Status { state, .. } = &event.event {
            if *state == CommunicationSessionState::Connected {
                self.next_reconnect = None;
            } else if matches!(
                state,
                CommunicationSessionState::Failed | CommunicationSessionState::Stopped
            ) && self.reconnect_armed
                && self.mode == TcpDebugMode::Client
            {
                self.next_reconnect = Some(
                    Instant::now()
                        + Duration::from_millis(self.reconnect_interval_ms.clamp(100, 60_000)),
                );
            }
        }
        self.common.handle_event(event);
        if self
            .selected_client
            .is_some_and(|id| !self.common.peers.iter().any(|peer| peer.id == id))
        {
            self.selected_client = None;
        }
        if self.mode == TcpDebugMode::Server && self.selected_client.is_none() {
            self.selected_client = self.common.peers.first().map(|peer| peer.id);
        }
    }

    fn set_busy(&mut self, _busy: bool) {}

    fn poll_actions(&mut self, now: Instant) -> Vec<AppAction> {
        if self.mode == TcpDebugMode::Client {
            if self.auto_reconnect && self.common.is_active() {
                self.reconnect_armed = true;
            } else if !self.auto_reconnect {
                self.reconnect_armed = false;
                self.next_reconnect = None;
            }
        }
        let send_target = self.send_target();
        let mut actions = self
            .common
            .poll_periodic(now, CommunicationKind::Tcp, send_target);
        if self.reconnect_armed && self.next_reconnect.is_some_and(|due| now >= due) {
            let config = self.config();
            if config.validate().is_ok() {
                self.next_reconnect = None;
                self.common.begin_start();
                actions.push(AppAction::StartCommunication(CommunicationConfig::Tcp(
                    config,
                )));
            } else {
                self.reconnect_armed = false;
                self.next_reconnect = None;
            }
        }
        actions
    }
}

pub struct UdpDebugTool {
    special_mode: bool,
    family: CommunicationIpFamily,
    local_address: String,
    local_port: u16,
    remote_address: String,
    remote_port: u16,
    special_local_address: String,
    special_local_port: u16,
    special_remote_port: u16,
    broadcast_address: String,
    multicast_enabled: bool,
    multicast_group: String,
    multicast_interface: String,
    multicast_ttl: u32,
    multicast_loopback: bool,
    reply_selected_source: bool,
    selected_source: Option<SocketAddr>,
    sources: Vec<SocketAddr>,
    review_seeded: bool,
    common: CommonCommunicationState,
}

impl Default for UdpDebugTool {
    fn default() -> Self {
        Self {
            special_mode: false,
            family: CommunicationIpFamily::V4,
            local_address: "127.0.0.1".into(),
            local_port: 9001,
            remote_address: "127.0.0.1".into(),
            remote_port: 9002,
            special_local_address: "0.0.0.0".into(),
            special_local_port: 9002,
            special_remote_port: 9002,
            broadcast_address: "255.255.255.255".into(),
            multicast_enabled: false,
            multicast_group: "239.255.0.1".into(),
            multicast_interface: "0.0.0.0".into(),
            multicast_ttl: 1,
            multicast_loopback: true,
            reply_selected_source: false,
            selected_source: None,
            sources: Vec::new(),
            review_seeded: false,
            common: CommonCommunicationState::default(),
        }
    }
}

impl UdpDebugTool {
    fn config(&self) -> UdpDebugConfig {
        let broadcast = self.special_mode && !self.multicast_enabled;
        let (local_address, local_port, remote_address, remote_port) = if self.special_mode {
            (
                self.special_local_address.trim().to_owned(),
                self.special_local_port,
                if self.multicast_enabled {
                    self.multicast_group.trim().to_owned()
                } else {
                    self.broadcast_address.trim().to_owned()
                },
                self.special_remote_port,
            )
        } else {
            (
                self.local_address.trim().to_owned(),
                self.local_port,
                self.remote_address.trim().to_owned(),
                self.remote_port,
            )
        };
        UdpDebugConfig {
            family: if self.special_mode {
                CommunicationIpFamily::V4
            } else {
                self.family
            },
            local_address,
            local_port,
            remote_address,
            remote_port,
            broadcast,
            multicast: (self.special_mode && self.multicast_enabled).then(|| UdpMulticastConfig {
                group: self.multicast_group.trim().into(),
                interface: self.multicast_interface.trim().into(),
                ttl: self.multicast_ttl,
                loopback: self.multicast_loopback,
            }),
        }
    }

    fn send_target(&self) -> CommunicationSendTarget {
        if !self.special_mode && self.reply_selected_source {
            self.selected_source.map_or(
                CommunicationSendTarget::Default,
                CommunicationSendTarget::UdpSource,
            )
        } else {
            CommunicationSendTarget::Default
        }
    }

    fn render_session_controls(&mut self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let active = self.common.is_active();
        let start_label = if self.special_mode && self.multicast_enabled {
            "加入"
        } else {
            "绑定"
        };
        if ui
            .add_enabled_ui(!active, |ui| {
                ui::primary_button_sized(ui, start_label, [68.0, ui::CONTROL_HEIGHT])
            })
            .inner
            .clicked()
        {
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
        let stop_label = if self.special_mode && self.multicast_enabled {
            "离开"
        } else {
            "停止"
        };
        if ui
            .add_enabled_ui(active, |ui| {
                ui::danger_button_sized(ui, stop_label, [68.0, ui::CONTROL_HEIGHT])
            })
            .inner
            .clicked()
        {
            self.common.stop_periodic();
            actions.push(AppAction::StopCommunication(CommunicationKind::Udp));
        }
    }

    fn seed_review(&mut self, variant: UiReviewVariant) {
        self.review_seeded = true;
        self.special_mode = variant == UiReviewVariant::UdpSpecial;
        self.multicast_enabled = self.special_mode;
        self.common.state = CommunicationSessionState::Connected;
        self.common.status_detail = if self.special_mode {
            "已加入组播组".into()
        } else {
            "UDP 已绑定 127.0.0.1:9001".into()
        };
        self.common.send_input = "Hello UDP".into();
        self.common.append_crlf = true;
        self.common.auto_scroll = false;
        let (outgoing_endpoint, incoming_endpoint) = if self.special_mode {
            ("239.255.0.1:9002", "192.0.2.55:54321")
        } else {
            (
                "127.0.0.1:9001 → 127.0.0.1:9002",
                "127.0.0.1:9002 → 127.0.0.1:9001",
            )
        };
        self.common
            .log
            .push_batch(review_records(outgoing_endpoint, incoming_endpoint));
        if self.special_mode {
            self.multicast_interface = "192.0.2.10".into();
        }
        if !self.special_mode {
            let source = "127.0.0.1:9002".parse().expect("固定审查地址应有效");
            self.sources.push(source);
            self.selected_source = Some(source);
        }
    }
}

impl ToolModule for UdpDebugTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: "udp-debug",
            name: "UDP 调试",
            description: "UDP 单播、IPv4 广播与单组 IPv4 组播数据报调试",
            category: ToolCategory::Communication,
            icon: ToolIcon::UdpDebug,
            keywords: &["udp", "datagram", "broadcast", "multicast", "组播", "广播"],
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, context: ToolUiContext) -> Vec<AppAction> {
        if context.review_mode && !self.review_seeded {
            self.seed_review(context.review_variant);
        }
        let mut actions = Vec::new();
        ui::page_heading(ui, "UDP 调试", "");
        ui.add_space(ui::SPACE_8);
        ui.add_enabled_ui(!self.common.is_active(), |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.special_mode, false, "普通收发");
                ui.selectable_value(&mut self.special_mode, true, "广播与组播");
            });
        });
        ui.add_space(ui::SPACE_12);
        ui::card(ui, |ui| {
            let active = self.common.is_active();
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.horizontal_wrapped(|ui| {
                if !self.special_mode {
                    ui.label("模式");
                    ui.add_enabled_ui(!active, |ui| {
                        for family in [CommunicationIpFamily::V4, CommunicationIpFamily::V6] {
                            ui.selectable_value(&mut self.family, family, family.label());
                        }
                    });
                }
                ui.label("本地绑定");
                if self.special_mode {
                    ui.add_enabled_ui(!active, |ui| {
                        ui.add_sized(
                            [142.0, ui::CONTROL_HEIGHT],
                            ui::text_input(&mut self.special_local_address, "本地地址"),
                        );
                    });
                    ui.label("本地端口");
                    ui.add_enabled_ui(!active, |ui| {
                        ui.add(egui::DragValue::new(&mut self.special_local_port));
                    });
                    ui.label("发送类型");
                    ui.add_enabled_ui(!active, |ui| {
                        ui.selectable_value(&mut self.multicast_enabled, false, "IPv4 广播");
                        ui.selectable_value(&mut self.multicast_enabled, true, "IPv4 组播");
                    });
                    ui.label(if self.multicast_enabled {
                        "组播地址"
                    } else {
                        "广播地址"
                    });
                    ui.add_enabled_ui(!active, |ui| {
                        if self.multicast_enabled {
                            ui.add_sized(
                                [150.0, ui::CONTROL_HEIGHT],
                                ui::text_input(&mut self.multicast_group, "IPv4 组播地址"),
                            );
                        } else {
                            ui.add_sized(
                                [150.0, ui::CONTROL_HEIGHT],
                                ui::text_input(&mut self.broadcast_address, "IPv4 广播地址"),
                            );
                        }
                    });
                    ui.label("远端端口");
                    ui.add_enabled_ui(!active, |ui| {
                        ui.add(egui::DragValue::new(&mut self.special_remote_port));
                    });
                } else {
                    ui.add_enabled_ui(!active, |ui| {
                        ui.add_sized(
                            [142.0, ui::CONTROL_HEIGHT],
                            ui::text_input(&mut self.local_address, "本地地址"),
                        );
                    });
                    ui.label("本地端口");
                    ui.add_enabled_ui(!active, |ui| {
                        ui.add(egui::DragValue::new(&mut self.local_port));
                    });
                    ui.label("默认远端");
                    ui.add_enabled_ui(!active, |ui| {
                        ui.add_sized(
                            [142.0, ui::CONTROL_HEIGHT],
                            ui::text_input(&mut self.remote_address, "远端地址"),
                        );
                    });
                    ui.label("远端端口");
                    ui.add_enabled_ui(!active, |ui| {
                        ui.add(egui::DragValue::new(&mut self.remote_port));
                    });
                }
            });
            if self.special_mode {
                ui.add_space(ui::SPACE_8);
                ui.separator();
                ui.add_space(ui::SPACE_8);
                ui.horizontal_wrapped(|ui| {
                    if self.multicast_enabled {
                        ui.label("本地接口");
                        ui.add_enabled_ui(!active, |ui| {
                            ui.add_sized(
                                [154.0, ui::CONTROL_HEIGHT],
                                ui::text_input(&mut self.multicast_interface, "本地 IPv4 接口"),
                            );
                        });
                        ui.label("TTL");
                        ui.add_enabled_ui(!active, |ui| {
                            ui.add(egui::DragValue::new(&mut self.multicast_ttl).range(1..=255));
                            ui::toggle_switch(ui, &mut self.multicast_loopback, "本机回环");
                        });
                    }
                    ui.separator();
                    self.common.render_inline_state(ui);
                    self.render_session_controls(ui, &mut actions);
                });
            } else {
                ui.add_space(ui::SPACE_8);
                ui.separator();
                ui.add_space(ui::SPACE_8);
                ui.horizontal_wrapped(|ui| {
                    ui.label("接收来源");
                    let selected = self
                        .selected_source
                        .map_or_else(|| "尚无来源".into(), |source| source.to_string());
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
                    ui.separator();
                    ui.label("发送目标");
                    ui.radio_value(&mut self.reply_selected_source, false, "默认远端");
                    ui.add_enabled_ui(self.selected_source.is_some(), |ui| {
                        ui.radio_value(&mut self.reply_selected_source, true, "回复选中来源");
                    });
                    ui.separator();
                    self.common.render_inline_state(ui);
                    self.render_session_controls(ui, &mut actions);
                });
            }
        });
        self.common.render_error(ui);
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
        if self.selected_source.is_none() {
            self.reply_selected_source = false;
        }
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
    rts: bool,
    dtr: bool,
    refresh_requested: bool,
    refresh_busy: bool,
    review_seeded: bool,
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
            rts: false,
            dtr: false,
            refresh_requested: true,
            refresh_busy: false,
            review_seeded: false,
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

    fn seed_review(&mut self) {
        self.review_seeded = true;
        self.refresh_requested = false;
        self.refresh_busy = false;
        self.selected_port = "COM3".into();
        self.ports = vec![SerialPortDescriptor {
            port_name: "COM3".into(),
            port_type: "USB".into(),
            manufacturer: Some("ToolDeck".into()),
            product: Some("USB Serial".into()),
            serial_number: None,
            vid: Some(0x1234),
            pid: Some(0x5678),
        }];
        self.common.state = CommunicationSessionState::Connected;
        self.common.status_detail = "设备已连接".into();
        self.common.send_input = "Hello World".into();
        self.common.append_crlf = true;
        self.common.auto_scroll = false;
        self.rts = true;
        self.dtr = true;
        self.common.log.push_batch(review_records("COM3", "COM3"));
    }
}

impl ToolModule for SerialDebugTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: "serial-debug",
            name: "串口调试",
            description: "独占打开 COM 口并持续进行文本或 HEX 收发",
            category: ToolCategory::Communication,
            icon: ToolIcon::SerialDebug,
            keywords: &["serial", "com", "uart", "串口", "波特率", "调试"],
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, context: ToolUiContext) -> Vec<AppAction> {
        if context.review_mode && !self.review_seeded {
            self.seed_review();
        }
        let mut actions = Vec::new();
        ui::page_heading(ui, "串口调试", "");
        ui.add_space(ui::SPACE_12);
        ui::card(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.horizontal_wrapped(|ui| {
                let active = self.common.is_active();
                ui.label("端口");
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
                ui.add_enabled_ui(!active, |ui| {
                    egui::ComboBox::from_id_salt("serial-debug-port")
                        .width(112.0)
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
                });
                if ui::secondary_button(
                    ui,
                    if self.refresh_busy {
                        "刷新中..."
                    } else {
                        "刷新"
                    },
                )
                .clicked()
                {
                    self.refresh_requested = true;
                }
                ui.label("波特率");
                ui.add_enabled_ui(!active, |ui| {
                    ui.add(egui::DragValue::new(&mut self.baud_rate).range(1..=4_000_000));
                });
                ui.label("数据位");
                ui.add_enabled_ui(!active, |ui| {
                    egui::ComboBox::from_id_salt("serial-data-bits")
                        .width(58.0)
                        .selected_text(self.data_bits.label())
                        .show_ui(ui, |ui| {
                            for value in SerialDataBits::ALL {
                                ui.selectable_value(&mut self.data_bits, value, value.label());
                            }
                        });
                });
                ui.label("校验位");
                ui.add_enabled_ui(!active, |ui| {
                    egui::ComboBox::from_id_salt("serial-parity")
                        .width(58.0)
                        .selected_text(self.parity.label())
                        .show_ui(ui, |ui| {
                            for value in SerialParity::ALL {
                                ui.selectable_value(&mut self.parity, value, value.label());
                            }
                        });
                });
                ui.label("停止位");
                ui.add_enabled_ui(!active, |ui| {
                    egui::ComboBox::from_id_salt("serial-stop-bits")
                        .width(58.0)
                        .selected_text(self.stop_bits.label())
                        .show_ui(ui, |ui| {
                            for value in SerialStopBits::ALL {
                                ui.selectable_value(&mut self.stop_bits, value, value.label());
                            }
                        });
                });
                ui.label("流控");
                ui.add_enabled_ui(!active, |ui| {
                    egui::ComboBox::from_id_salt("serial-flow-control")
                        .width(58.0)
                        .selected_text(self.flow_control.label())
                        .show_ui(ui, |ui| {
                            for value in SerialFlowControl::ALL {
                                ui.selectable_value(&mut self.flow_control, value, value.label());
                            }
                        });
                });
                ui.add_sized([76.0, ui::CONTROL_HEIGHT], egui::Label::new("读超时(ms)"));
                ui.add_enabled_ui(!active, |ui| {
                    ui.add(egui::DragValue::new(&mut self.read_timeout_ms).range(10..=1_000));
                });
            });
            ui.add_space(ui::SPACE_8);
            ui.separator();
            ui.add_space(ui::SPACE_8);
            ui.horizontal_wrapped(|ui| {
                let active = self.common.is_active();
                if ui
                    .add_enabled_ui(!active, |ui| {
                        ui::primary_button_sized(ui, "打开", [68.0, ui::CONTROL_HEIGHT])
                    })
                    .inner
                    .clicked()
                {
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
                if ui
                    .add_enabled_ui(active, |ui| {
                        ui::danger_button_sized(ui, "关闭", [68.0, ui::CONTROL_HEIGHT])
                    })
                    .inner
                    .clicked()
                {
                    self.common.stop_periodic();
                    actions.push(AppAction::StopCommunication(CommunicationKind::Serial));
                }
                ui.separator();
                if ui
                    .add_enabled_ui(active, |ui| ui::toggle_switch(ui, &mut self.rts, "RTS"))
                    .inner
                    .changed()
                {
                    actions.push(AppAction::SendCommunication {
                        kind: CommunicationKind::Serial,
                        command: CommunicationCommand::SetRts(self.rts),
                    });
                }
                if ui
                    .add_enabled_ui(active, |ui| ui::toggle_switch(ui, &mut self.dtr, "DTR"))
                    .inner
                    .changed()
                {
                    actions.push(AppAction::SendCommunication {
                        kind: CommunicationKind::Serial,
                        command: CommunicationCommand::SetDtr(self.dtr),
                    });
                }
                ui.separator();
                let endpoint = if self.selected_port.is_empty() {
                    "未选择"
                } else {
                    &self.selected_port
                };
                self.common.render_status(ui, "SERIAL", endpoint, false);
                if !self.common.status_detail.is_empty() {
                    ui.separator();
                    ui.label(
                        RichText::new(&self.common.status_detail)
                            .color(ui::palette_for_ui(ui).weak),
                    );
                }
            });
        });
        self.common.render_error(ui);
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
        if matches!(
            &event.event,
            CommunicationEvent::Status {
                state: CommunicationSessionState::Failed | CommunicationSessionState::Stopped,
                ..
            }
        ) {
            self.rts = false;
            self.dtr = false;
        }
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
    if viewport_height < DESIGN_VIEWPORT_HEIGHT {
        ((viewport_height - 640.0) * 0.275 + COMPACT_LOG_HEIGHT).clamp(COMPACT_LOG_HEIGHT, 220.0)
    } else {
        (viewport_height * LOG_HEIGHT_RATIO).clamp(280.0, MAX_LOG_HEIGHT)
    }
}

fn log_toolbar_action(
    ui: &mut egui::Ui,
    icon: ui::AppIcon,
    label: &str,
    tooltip: &str,
) -> bool {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        let icon_clicked = ui::icon_button_sized(ui, icon, tooltip, false, ui::COMPACT_CONTROL_HEIGHT)
            .clicked();
        let label_clicked = ui
            .add(
                egui::Label::new(RichText::new(label).size(12.0))
                    .sense(egui::Sense::click()),
            )
            .clicked();
        icon_clicked || label_clicked
    })
    .inner
}

fn format_segment(ui: &mut egui::Ui, format: &mut PayloadFormat, id: (&'static str, &'static str)) {
    ui.push_id(id, |ui| {
        ui.horizontal(|ui| {
            for value in [PayloadFormat::Text, PayloadFormat::Hex] {
                ui.selectable_value(format, value, value.label());
            }
        });
    });
}

fn format_bytes(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn format_elapsed(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let hours = total_seconds / 3_600;
    let minutes = total_seconds % 3_600 / 60;
    let seconds = total_seconds % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

fn record_columns(available_width: f32) -> [f32; 5] {
    let content_width = available_width.max(540.0);
    if available_width < 760.0 {
        let endpoint = (content_width * 0.22).clamp(140.0, 170.0);
        let data = (content_width - 120.0 - 68.0 - endpoint - 72.0).max(200.0);
        [120.0, 68.0, endpoint, 72.0, data]
    } else {
        let endpoint = (content_width * 0.27).clamp(220.0, 300.0);
        let data = (content_width - 132.0 - 78.0 - endpoint - 82.0).max(220.0);
        [132.0, 78.0, endpoint, 82.0, data]
    }
}

fn render_record_header(ui: &mut egui::Ui, height: f32) {
    let widths = record_columns(ui.available_width());
    let (response, painter) = ui.allocate_painter(
        egui::vec2(widths.iter().sum(), height),
        egui::Sense::hover(),
    );
    let palette = ui::palette_for_ui(ui);
    let rect = response.rect;
    painter.rect_filled(rect, egui::CornerRadius::ZERO, palette.surface_elevated);
    paint_record_grid(&painter, rect, &widths, palette.border_subtle);
    for (index, label) in ["本地时间 (ms)", "方向", "端点", "字节数", "数据"]
        .iter()
        .enumerate()
    {
        paint_record_text(
            &painter,
            record_cell(rect, &widths, index),
            label,
            egui::FontId::proportional(14.0),
            palette.text,
        );
    }
}

fn render_record(
    ui: &mut egui::Ui,
    record: &CommunicationRecord,
    format: PayloadFormat,
    row_height: f32,
) {
    let palette = ui::palette_for_ui(ui);
    let direction_color = match record.direction {
        CommunicationDirection::Incoming => palette.receive_text,
        CommunicationDirection::Outgoing => palette.success_text,
        CommunicationDirection::Status => palette.warning_text,
    };
    let content = record
        .note
        .clone()
        .unwrap_or_else(|| render_payload(&record.payload, format));
    let widths = record_columns(ui.available_width());
    let (response, painter) = ui.allocate_painter(
        egui::vec2(widths.iter().sum(), row_height),
        egui::Sense::hover(),
    );
    let rect = response.rect;
    painter.rect_filled(rect, egui::CornerRadius::ZERO, palette.surface);
    paint_record_grid(&painter, rect, &widths, palette.border_subtle);
    paint_record_text(
        &painter,
        record_cell(rect, &widths, 0),
        &record.timestamp,
        egui::FontId::monospace(13.5),
        palette.text,
    );
    paint_record_text(
        &painter,
        record_cell(rect, &widths, 1),
        record_direction_label(record.direction),
        egui::FontId::proportional(14.0),
        direction_color,
    );
    paint_record_text(
        &painter,
        record_cell(rect, &widths, 2),
        &record.endpoint,
        egui::FontId::monospace(13.5),
        palette.text,
    );
    paint_record_text(
        &painter,
        record_cell(rect, &widths, 3),
        &record.payload.len().to_string(),
        egui::FontId::monospace(13.5),
        palette.text,
    );
    paint_record_text(
        &painter,
        record_cell(rect, &widths, 4),
        &content,
        egui::FontId::monospace(13.5),
        direction_color,
    );
}

fn record_cell(row: egui::Rect, widths: &[f32], index: usize) -> egui::Rect {
    let left = row.left() + widths[..index].iter().sum::<f32>();
    egui::Rect::from_min_size(
        egui::pos2(left, row.top()),
        egui::vec2(widths[index], row.height()),
    )
}

fn paint_record_grid(
    painter: &egui::Painter,
    row: egui::Rect,
    widths: &[f32],
    border: egui::Color32,
) {
    painter.rect_stroke(
        row,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.0_f32, border),
        egui::StrokeKind::Middle,
    );
    let mut x = row.left();
    for width in widths.iter().take(widths.len().saturating_sub(1)) {
        x += *width;
        painter.line_segment(
            [egui::pos2(x, row.top()), egui::pos2(x, row.bottom())],
            egui::Stroke::new(1.0_f32, border),
        );
    }
}

fn paint_record_text(
    painter: &egui::Painter,
    cell: egui::Rect,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
) {
    painter
        .with_clip_rect(cell.shrink2(egui::vec2(10.0, 2.0)))
        .text(
            cell.left_center() + egui::vec2(12.0, 0.0),
            egui::Align2::LEFT_CENTER,
            text,
            font,
            color,
        );
}

fn record_direction_label(direction: CommunicationDirection) -> &'static str {
    match direction {
        CommunicationDirection::Incoming => "↓ 接收",
        CommunicationDirection::Outgoing => "↑ 发送",
        CommunicationDirection::Status => "状态",
    }
}

fn render_tcp_peer_list(
    ui: &mut egui::Ui,
    peers: &[CommunicationPeer],
    selected_client: &mut Option<u64>,
) {
    let palette = ui::palette_for_ui(ui);
    let height = if peers.is_empty() {
        communication_log_height(ui.ctx().screen_rect().height())
    } else {
        communication_log_height(ui.ctx().screen_rect().height()).max(360.0)
    };
    ui::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("客户端列表").strong().size(14.5));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(format!("{} / 32", peers.len()));
            });
        });
        ui.add_space(ui::SPACE_8);
        let widths = peer_columns(ui.available_width());
        egui::ScrollArea::both()
            .max_height(height)
            .min_scrolled_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (label, width) in [
                        ("端点", widths[0]),
                        ("已连接时间", widths[1]),
                        ("接收字节", widths[2]),
                        ("发送字节", widths[3]),
                        ("选中", widths[4]),
                    ] {
                        ui.add_sized(
                            [width, 28.0],
                            egui::Label::new(RichText::new(label).strong()),
                        );
                    }
                });
                ui.separator();
                ui.set_min_height(height);
                for peer in peers {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [widths[0], 34.0],
                            egui::Label::new(
                                RichText::new(&peer.endpoint)
                                    .monospace()
                                    .color(palette.text),
                            )
                            .truncate(),
                        );
                        ui.add_sized(
                            [widths[1], 34.0],
                            egui::Label::new(RichText::new(&peer.connected_at).monospace())
                                .truncate(),
                        );
                        ui.add_sized(
                            [widths[2], 34.0],
                            egui::Label::new(
                                RichText::new(format_u64_bytes(peer.received_bytes)).monospace(),
                            ),
                        );
                        ui.add_sized(
                            [widths[3], 34.0],
                            egui::Label::new(
                                RichText::new(format_u64_bytes(peer.sent_bytes)).monospace(),
                            ),
                        );
                        if ui
                            .add_sized(
                                [widths[4], 34.0],
                                egui::RadioButton::new(*selected_client == Some(peer.id), ""),
                            )
                            .clicked()
                        {
                            *selected_client = Some(peer.id);
                        }
                    });
                    ui.separator();
                }
            });
    });
}

fn peer_columns(available_width: f32) -> [f32; 5] {
    let content_width = (available_width - ui::SPACE_8 * 4.0).max(300.0);
    if available_width < 520.0 {
        [
            content_width * 0.34,
            content_width * 0.26,
            content_width * 0.14,
            content_width * 0.14,
            content_width * 0.12,
        ]
    } else {
        let endpoint = (content_width - 112.0 - 82.0 - 82.0 - 48.0).max(180.0);
        [endpoint, 112.0, 82.0, 82.0, 48.0]
    }
}

fn format_u64_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} B")
    }
}

fn review_records(outgoing_endpoint: &str, incoming_endpoint: &str) -> Vec<CommunicationRecord> {
    [
        (
            "14:22:31.123",
            CommunicationDirection::Outgoing,
            b"Hello World\r\n".as_slice(),
        ),
        (
            "14:22:31.124",
            CommunicationDirection::Incoming,
            b"Hello World\r\n".as_slice(),
        ),
        (
            "14:22:33.501",
            CommunicationDirection::Outgoing,
            b"PING\r\n".as_slice(),
        ),
        (
            "14:22:33.502",
            CommunicationDirection::Incoming,
            b"PONG\r\n".as_slice(),
        ),
        (
            "14:22:37.815",
            CommunicationDirection::Outgoing,
            b"{\"type\":\"ping\",\"id\":1}\r\n".as_slice(),
        ),
        (
            "14:22:37.816",
            CommunicationDirection::Incoming,
            b"{\"type\":\"pong\",\"id\":1}\r\n".as_slice(),
        ),
        (
            "14:22:40.102",
            CommunicationDirection::Incoming,
            b"Welcome to ToolDeck\r\n".as_slice(),
        ),
        (
            "14:22:45.332",
            CommunicationDirection::Outgoing,
            b"Time: 2025-05-24\r\n".as_slice(),
        ),
    ]
    .into_iter()
    .map(|(timestamp, direction, payload)| CommunicationRecord {
        timestamp: timestamp.into(),
        direction,
        endpoint: match direction {
            CommunicationDirection::Incoming => incoming_endpoint.into(),
            CommunicationDirection::Outgoing | CommunicationDirection::Status => {
                outgoing_endpoint.into()
            }
        },
        payload: payload.to_vec(),
        note: None,
    })
    .collect()
}

fn review_server_records(
    outgoing_endpoint: &str,
    incoming_endpoint: &str,
) -> Vec<CommunicationRecord> {
    let mut records = review_records(outgoing_endpoint, incoming_endpoint);
    records.extend(
        [
            (
                "14:24:01.221",
                CommunicationDirection::Outgoing,
                b"ping from client2\r\n".as_slice(),
            ),
            (
                "14:24:01.222",
                CommunicationDirection::Incoming,
                b"pong from server\r\n".as_slice(),
            ),
            (
                "14:24:15.558",
                CommunicationDirection::Incoming,
                b"hello from server\r\n".as_slice(),
            ),
        ]
        .into_iter()
        .map(|(timestamp, direction, payload)| CommunicationRecord {
            timestamp: timestamp.into(),
            direction,
            endpoint: match direction {
                CommunicationDirection::Incoming => incoming_endpoint.into(),
                CommunicationDirection::Outgoing | CommunicationDirection::Status => {
                    outgoing_endpoint.into()
                }
            },
            payload: payload.to_vec(),
            note: None,
        }),
    );
    records
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
        assert_eq!(communication_log_height(500.0), COMPACT_LOG_HEIGHT);
        assert_eq!(communication_log_height(640.0), COMPACT_LOG_HEIGHT);
        assert_eq!(communication_log_height(1_000.0), 290.0);
        assert_eq!(communication_log_height(3_000.0), MAX_LOG_HEIGHT);
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

    #[test]
    fn elapsed_time_uses_fixed_clock_format() {
        assert_eq!(
            format_elapsed(Duration::from_secs(12 * 60 + 48)),
            "00:12:48"
        );
        assert_eq!(format_elapsed(Duration::from_secs(3_661)), "01:01:01");
    }
}
