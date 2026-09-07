//! 端口占用工具模块。
//! 自动刷新仅在上一次请求完成后创建下一次后台任务，避免高频查询堆积到 UI 消息队列。

use std::{
    cmp::Ordering,
    time::{Duration, Instant},
};

use eframe::egui::{self, Color32, RichText};

use crate::{
    core::{
        actions::AppAction,
        invocation::{ToolInvocation, ToolPayload},
        worker::TaskResult,
    },
    model::{AppError, IpVersion, NetworkEndpoint, NetworkProtocol, ProcessSummary, TcpState},
    platform::windows::local_time_hms,
    tools::{
        ToolModule, ToolUiContext,
        file_lock::{empty_state, error_state, heading},
        registry::{ToolCategory, ToolDescriptor, ToolIcon},
    },
    ui,
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum RefreshInterval {
    Off,
    OneSecond,
    #[default]
    TwoSeconds,
    FiveSeconds,
    TenSeconds,
}

impl RefreshInterval {
    const ALL: [Self; 5] = [
        Self::Off,
        Self::OneSecond,
        Self::TwoSeconds,
        Self::FiveSeconds,
        Self::TenSeconds,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Off => "关闭",
            Self::OneSecond => "1 秒",
            Self::TwoSeconds => "2 秒",
            Self::FiveSeconds => "5 秒",
            Self::TenSeconds => "10 秒",
        }
    }

    fn duration(self) -> Option<Duration> {
        match self {
            Self::Off => None,
            Self::OneSecond => Some(Duration::from_secs(1)),
            Self::TwoSeconds => Some(Duration::from_secs(2)),
            Self::FiveSeconds => Some(Duration::from_secs(5)),
            Self::TenSeconds => Some(Duration::from_secs(10)),
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum SortColumn {
    #[default]
    None,
    Protocol,
    LocalAddress,
    RemoteAddress,
    State,
    Pid,
    Process,
}

#[derive(Default)]
pub struct PortInspectorTool {
    address_filter: String,
    exact_port: String,
    pid_filter: String,
    process_filter: String,
    exact_pid: Option<u32>,
    protocol: Option<NetworkProtocol>,
    ip_version: Option<IpVersion>,
    state: Option<TcpState>,
    sort_column: SortColumn,
    sort_ascending: bool,
    interval: RefreshInterval,
    paused: bool,
    endpoints: Vec<NetworkEndpoint>,
    error: Option<AppError>,
    busy: bool,
    last_refresh: Option<Instant>,
    last_refresh_label: Option<String>,
    review_seeded: bool,
    selected_endpoint: Option<String>,
    selected_process_pid: Option<u32>,
    selected_process: Option<Result<ProcessSummary, AppError>>,
}

impl ToolModule for PortInspectorTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: "port-inspector",
            name: "端口占用",
            description: "检查 TCP 和 UDP 端口、连接状态与所属进程",
            category: ToolCategory::Diagnostic,
            icon: ToolIcon::Network,
            keywords: &[
                "port", "tcp", "udp", "listen", "socket", "network", "端口", "占用", "网络",
            ],
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, context: ToolUiContext) -> Vec<AppAction> {
        if context.review_mode && !self.review_seeded {
            self.seed_review();
        }
        let mut actions = Vec::new();
        let palette = ui::palette_for_ui(ui);
        heading(ui, "端口占用", "查看本机 TCP 与 UDP 端点及所属进程");
        ui.add_space(ui::SPACE_16);

        ui::card(ui, |ui| {
            let compact_controls = ui.available_width() < 1_000.0;
            let segment_width = if compact_controls { 56.0 } else { 64.0 };
            let address_width = if compact_controls { 135.0 } else { 170.0 };
            let port_width = if compact_controls { 90.0 } else { 118.0 };
            let pid_width = if compact_controls { 90.0 } else { 118.0 };
            let process_width = if compact_controls { 140.0 } else { 176.0 };
            let field_gap = if compact_controls { ui::SPACE_8 } else { ui::SPACE_12 };
            let query_width = if compact_controls { 70.0 } else { 76.0 };
            let protocol_width = segment_width * 3.0;
            let field_widths = [
                protocol_width,
                address_width,
                port_width,
                pid_width,
                process_width,
                query_width,
            ];
            ui::responsive_parameter_row(ui, &field_widths, field_gap, |ui| {
                ui::parameter_group(ui, protocol_width, |ui| {
                    ui.allocate_space(egui::vec2(protocol_width, 16.0));
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for (label, value) in [
                            ("全部", None),
                            ("TCP", Some(NetworkProtocol::Tcp)),
                            ("UDP", Some(NetworkProtocol::Udp)),
                        ] {
                            ui.add_sized(
                                [segment_width, ui::CONTROL_HEIGHT],
                                egui::Button::selectable(self.protocol == value, label),
                            )
                            .clicked()
                            .then(|| self.protocol = value);
                        }
                    });
                });

                ui::parameter_group(ui, address_width, |ui| {
                    ui.add_sized(
                        [address_width, 16.0],
                        egui::Label::new(RichText::new("地址").size(12.0).color(palette.weak)),
                    );
                    ui.add_sized(
                        [address_width, ui::CONTROL_HEIGHT],
                        ui::text_input(&mut self.address_filter, "例如: 127.0.0.1"),
                    );
                });

                ui::parameter_group(ui, port_width, |ui| {
                    ui.add_sized(
                        [port_width, 16.0],
                        egui::Label::new(RichText::new("端口").size(12.0).color(palette.weak)),
                    );
                    ui.add_sized(
                        [port_width, ui::CONTROL_HEIGHT],
                        ui::text_input(&mut self.exact_port, "例如: 8080"),
                    );
                });

                ui::parameter_group(ui, pid_width, |ui| {
                    ui.add_sized(
                        [pid_width, 16.0],
                        egui::Label::new(RichText::new("PID").size(12.0).color(palette.weak)),
                    );
                    ui.add_sized(
                        [pid_width, ui::CONTROL_HEIGHT],
                        ui::text_input(&mut self.pid_filter, "例如: 1234"),
                    );
                });

                ui::parameter_group(ui, process_width, |ui| {
                    ui.add_sized(
                        [process_width, 16.0],
                        egui::Label::new(RichText::new("进程").size(12.0).color(palette.weak)),
                    );
                    ui.add_sized(
                        [process_width, ui::CONTROL_HEIGHT],
                        ui::text_input(&mut self.process_filter, "例如: chrome.exe"),
                    );
                });

                ui::parameter_group(ui, query_width, |ui| {
                    ui.allocate_space(egui::vec2(query_width, 16.0));
                    if ui::primary_button_sized(
                        ui,
                        "查询",
                        [query_width, ui::CONTROL_HEIGHT],
                    )
                    .clicked()
                    {
                        actions.push(AppAction::RefreshPorts);
                    }
                });
            });
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label("自动刷新");
                for interval in RefreshInterval::ALL {
                    ui.add_sized(
                        [60.0, ui::CONTROL_HEIGHT],
                        egui::Button::selectable(self.interval == interval, interval.label()),
                    )
                    .clicked()
                    .then(|| self.interval = interval);
                }
                let action_width = 176.0;
                ui.add_space((ui.available_width() - action_width).max(0.0));
                if ui::secondary_button(ui, if self.paused { "▶ 继续" } else { "❚❚ 暂停" }).clicked() {
                    self.paused = !self.paused;
                }
                ui.add_space(ui::SPACE_8);
                if ui::secondary_button(ui, "⤤ 导出 CSV").clicked() {
                    let rows = self.filtered_rows();
                    actions.push(AppAction::ExportPortsCsv {
                        content: endpoints_csv(&rows),
                    });
                }
            });
            if let Some(pid) = self.exact_pid {
                ui.add_space(ui::SPACE_8);
                ui.horizontal(|ui| {
                    ui::badge(
                        ui,
                        &format!("已过滤 PID: {pid}"),
                        palette.accent,
                        palette.accent.gamma_multiply(if ui.visuals().dark_mode {
                            0.22
                        } else {
                            0.12
                        }),
                    );
                    if ui::small_action_button(ui, "清除过滤").clicked() {
                        self.exact_pid = None;
                    }
                });
            }
        });
        ui.add_space(ui::SPACE_16);

        if self.busy && self.endpoints.is_empty() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    RichText::new("正在通过 IP Helper 读取系统网络连接与端口表...").size(13.5),
                );
            });
        }
        if let Some(error) = &self.error {
            error_state(ui, error);
        } else if self.endpoints.is_empty() && !self.busy {
            empty_state(
                ui,
                "暂未获取到端口数据",
                "点击“立即刷新”，或开启定时自动刷新以持续监控网络端口占用。",
            )
        } else {
            self.render_table(ui, &mut actions);
        }
        actions
    }

    fn handle_invocation(&mut self, payload: ToolPayload) -> Vec<AppAction> {
        match payload {
            ToolPayload::Port { port } => {
                self.exact_port = port.to_string();
                self.exact_pid = None;
                vec![AppAction::RefreshPorts]
            }
            ToolPayload::Process { pid } => {
                self.exact_pid = Some(pid);
                vec![AppAction::RefreshPorts]
            }
            _ => Vec::new(),
        }
    }

    fn handle_task_result(&mut self, result: TaskResult) {
        match result {
            TaskResult::Ports(result) => {
                self.busy = false;
                self.last_refresh = Some(Instant::now());
                self.last_refresh_label = Some(local_time_hms());
                match result {
                    Ok(endpoints) => {
                        self.endpoints = endpoints;
                        self.error = None;
                    }
                    Err(error) => self.error = Some(error),
                }
            }
            TaskResult::PortProcessDetails { pid, result }
                if self.selected_process_pid == Some(pid) =>
            {
                self.selected_process = Some(result);
            }
            _ => {}
        }
    }

    fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    fn poll_actions(&mut self, now: Instant) -> Vec<AppAction> {
        self.due_refresh(now)
            .then_some(AppAction::RefreshPorts)
            .into_iter()
            .collect()
    }
}

impl PortInspectorTool {
    fn seed_review(&mut self) {
        self.review_seeded = true;
        self.endpoints.clear();
        self.endpoints.extend([
            review_endpoint(
                NetworkProtocol::Tcp,
                IpVersion::V4,
                "0.0.0.0",
                80,
                "0.0.0.0",
                Some(0),
                Some(TcpState::Listen),
                1234,
                "nginx.exe",
            ),
            review_endpoint(
                NetworkProtocol::Tcp,
                IpVersion::V4,
                "127.0.0.1",
                8080,
                "127.0.0.1",
                Some(52314),
                Some(TcpState::Established),
                5678,
                "ToolDeck.exe",
            ),
            review_endpoint(
                NetworkProtocol::Tcp,
                IpVersion::V4,
                "127.0.0.1",
                9000,
                "127.0.0.1",
                Some(52315),
                Some(TcpState::Established),
                2222,
                "chrome.exe",
            ),
            review_endpoint(
                NetworkProtocol::Tcp,
                IpVersion::V4,
                "127.0.0.1",
                3306,
                "127.0.0.1",
                Some(52316),
                Some(TcpState::Established),
                4180,
                "mysqld.exe",
            ),
            review_endpoint(
                NetworkProtocol::Tcp,
                IpVersion::V6,
                "::1",
                5432,
                "::1",
                Some(52317),
                Some(TcpState::Established),
                3360,
                "postgres.exe",
            ),
            review_endpoint(
                NetworkProtocol::Tcp,
                IpVersion::V4,
                "0.0.0.0",
                22,
                "0.0.0.0",
                Some(0),
                Some(TcpState::Listen),
                1000,
                "sshd.exe",
            ),
            review_endpoint(
                NetworkProtocol::Udp,
                IpVersion::V4,
                "0.0.0.0",
                53,
                "0.0.0.0",
                Some(0),
                None,
                1234,
                "dns.exe",
            ),
            review_endpoint(
                NetworkProtocol::Udp,
                IpVersion::V4,
                "127.0.0.1",
                123,
                "0.0.0.0",
                Some(0),
                None,
                1560,
                "w32time.exe",
            ),
            review_endpoint(
                NetworkProtocol::Udp,
                IpVersion::V6,
                "::1",
                5333,
                "0.0.0.0",
                Some(0),
                None,
                3456,
                "svchost.exe",
            ),
            review_endpoint(
                NetworkProtocol::Tcp,
                IpVersion::V4,
                "127.0.0.1",
                6379,
                "127.0.0.1",
                Some(52318),
                Some(TcpState::Established),
                2890,
                "redis-server.exe",
            ),
        ]);
        for index in 0..16_u16 {
            self.endpoints.push(review_endpoint(
                NetworkProtocol::Tcp,
                IpVersion::V4,
                "0.0.0.0",
                10000 + index,
                "0.0.0.0",
                Some(0),
                Some(TcpState::Listen),
                3000 + u32::from(index),
                &format!("service-{index}.exe"),
            ));
        }
        self.endpoints.push(review_endpoint(
            NetworkProtocol::Tcp,
            IpVersion::V4,
            "127.0.0.1",
            6380,
            "127.0.0.1",
            Some(52319),
            Some(TcpState::Established),
            2891,
            "redis-monitor.exe",
        ));
        for index in 0..9_u16 {
            self.endpoints.push(review_endpoint(
                NetworkProtocol::Udp,
                IpVersion::V4,
                "127.0.0.1",
                9100 + index,
                "0.0.0.0",
                Some(0),
                None,
                4000 + u32::from(index),
                &format!("udp-{index}.exe"),
            ));
        }
        self.last_refresh = Some(Instant::now());
        self.last_refresh_label = Some("14:25:10".into());
        self.interval = RefreshInterval::Off;
        if let Some(endpoint) = self.endpoints.get(1) {
            self.selected_endpoint = Some(endpoint_report_line(endpoint));
            self.selected_process_pid = Some(endpoint.pid);
            self.selected_process = Some(Ok(ProcessSummary {
                pid: endpoint.pid,
                name: endpoint.process_name.clone(),
                exe_path: Some(r"C:\Program Files\ToolDeck\ToolDeck.exe".into()),
                owner: Some("DESKTOP\\chen".into()),
                started_at: Some("2025-05-20 11:45:01".into()),
                ..Default::default()
            }));
        }
    }

    pub fn due_refresh(&self, now: Instant) -> bool {
        self.interval.duration().is_some_and(|interval| {
            !self.busy
                && !self.paused
                && self
                    .last_refresh
                    .is_none_or(|last| now.duration_since(last) >= interval)
        })
    }

    fn filtered_rows(&self) -> Vec<&NetworkEndpoint> {
        let address = self.address_filter.trim().to_lowercase();
        let process = self.process_filter.trim().to_lowercase();
        let pid = self.pid_filter.trim();
        let exact_port = self
            .exact_port
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0);
        if !self.exact_port.trim().is_empty() && exact_port.is_none() {
            return Vec::new();
        }
        self.endpoints
            .iter()
            .filter(|endpoint| {
                self.protocol
                    .is_none_or(|protocol| endpoint.protocol == protocol)
            })
            .filter(|endpoint| {
                self.ip_version
                    .is_none_or(|version| endpoint.ip_version == version)
            })
            .filter(|endpoint| self.state.is_none_or(|state| endpoint.state == Some(state)))
            .filter(|endpoint| {
                exact_port.is_none_or(|port| {
                    endpoint.local_port == port || endpoint.remote_port == Some(port)
                })
            })
            .filter(|endpoint| self.exact_pid.is_none_or(|value| endpoint.pid == value))
            .filter(|endpoint| {
                address.is_empty()
                    || endpoint.local_address.to_lowercase().contains(&address)
                    || endpoint.remote_address.to_lowercase().contains(&address)
            })
            .filter(|endpoint| pid.is_empty() || endpoint.pid.to_string().contains(pid))
            .filter(|endpoint| {
                process.is_empty() || endpoint.process_name.to_lowercase().contains(&process)
            })
            .collect()
    }

    fn render_table(&mut self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let invalid_port = !self.exact_port.trim().is_empty()
            && self
                .exact_port
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|port| *port > 0)
                .is_none();
        let mut sort_column = self.sort_column;
        let mut sort_ascending = self.sort_ascending;
        let mut selected_endpoint = self.selected_endpoint.clone();
        let mut rows = self.filtered_rows();
        sort_endpoints(&mut rows, sort_column, sort_ascending);
        if selected_endpoint.as_ref().is_none_or(|selected| {
            !rows
                .iter()
                .any(|row| endpoint_report_line(row) == *selected)
        }) {
            selected_endpoint = rows.first().map(|row| endpoint_report_line(row));
        }

        let listening_count = rows
            .iter()
            .filter(|r| r.state == Some(TcpState::Listen))
            .count();
        let established_count = rows
            .iter()
            .filter(|r| r.state == Some(TcpState::Established))
            .count();
        let tcp_count = rows
            .iter()
            .filter(|row| row.protocol == NetworkProtocol::Tcp)
            .count();
        let udp_count = rows
            .iter()
            .filter(|row| row.protocol == NetworkProtocol::Udp)
            .count();

        render_port_statistics(
            ui,
            [
                ("监听", listening_count),
                ("已建立", established_count),
                ("TCP", tcp_count),
                ("UDP", udp_count),
            ],
        );
        if invalid_port {
            ui.add_space(ui::SPACE_8);
            ui.label(RichText::new("精确端口必须是 1 到 65535 的整数").color(ui::danger_text(ui)));
        }
        ui.add_space(ui::SPACE_12);
        let table_height = (ui.ctx().screen_rect().height() * 0.34).clamp(280.0, 360.0);
        ui::table_card(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("port-inspector-table-scroll")
                .max_height(table_height)
                .min_scrolled_height(table_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_min_height(table_height);
                    render_port_rows(
                        ui,
                        &rows,
                        actions,
                        &mut sort_column,
                        &mut sort_ascending,
                        &mut selected_endpoint,
                    )
                });
        });
        let selected_pid = rows.iter().find(|row| {
            selected_endpoint
                .as_ref()
                .is_some_and(|selected| endpoint_report_line(row) == *selected)
        });
        if let Some(endpoint) = selected_pid {
            ui.add_space(ui::SPACE_12);
            let process = (self.selected_process_pid == Some(endpoint.pid))
                .then_some(self.selected_process.as_ref())
                .flatten();
            render_selected_endpoint(ui, endpoint, process, actions);
        }
        let selected_pid = selected_pid.map(|endpoint| endpoint.pid);
        drop(rows);
        if self.selected_process_pid != selected_pid {
            self.selected_process_pid = selected_pid;
            self.selected_process = None;
            if let Some(pid) = selected_pid {
                actions.push(AppAction::LoadPortProcessDetails { pid });
            }
        }
        self.sort_column = sort_column;
        self.sort_ascending = sort_ascending;
        self.selected_endpoint = selected_endpoint;
    }
}

/// 绘制筛选项的标签与输入框，保证所有字段在同一标签基线和控件基线上对齐。
#[allow(dead_code)]
fn render_filter_input(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    hint: &str,
    width: f32,
) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).size(13.0));
        ui.add_space(4.0);
        ui.add_sized([width, ui::CONTROL_HEIGHT], ui::text_input(value, hint));
    });
}

/// 绘制端口页的单一统计条，使端点统计保持连续而非拆分为独立信息卡。
fn render_port_statistics(ui: &mut egui::Ui, statistics: [(&str, usize); 4]) {
    const HEIGHT: f32 = 68.0;
    let palette = ui::palette_for_ui(ui);
    let (response, painter) = ui.allocate_painter(
        egui::vec2(ui.available_width(), HEIGHT),
        egui::Sense::hover(),
    );
    let rect = response.rect;
    painter.rect(
        rect,
        egui::CornerRadius::same(6),
        palette.surface,
        egui::Stroke::new(1.0_f32, palette.border),
        egui::StrokeKind::Middle,
    );

    let cell_width = rect.width() / statistics.len() as f32;
    for (index, (label, value)) in statistics.into_iter().enumerate() {
        let cell_left = rect.left() + cell_width * index as f32;
        let cell_center = cell_left + cell_width * 0.5;
        if index > 0 {
            painter.line_segment(
                [
                    egui::pos2(cell_left, rect.top() + 8.0),
                    egui::pos2(cell_left, rect.bottom() - 8.0),
                ],
                egui::Stroke::new(1.0_f32, palette.border),
            );
        }
        painter.text(
            egui::pos2(cell_center, rect.top() + 14.0),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(13.0),
            palette.weak,
        );
        painter.text(
            egui::pos2(cell_center, rect.top() + 43.0),
            egui::Align2::CENTER_CENTER,
            value.to_string(),
            egui::FontId::monospace(22.0),
            palette.accent,
        );
    }
}

/// 构造视觉审查专用端点，不参与运行时的系统端口读取和过滤数据来源。
#[allow(
    clippy::too_many_arguments,
    reason = "视觉审查端点字段在调用点完整列出以便逐行核对"
)]
fn review_endpoint(
    protocol: NetworkProtocol,
    ip_version: IpVersion,
    local_address: &str,
    local_port: u16,
    remote_address: &str,
    remote_port: Option<u16>,
    state: Option<TcpState>,
    pid: u32,
    process_name: &str,
) -> NetworkEndpoint {
    NetworkEndpoint {
        protocol,
        ip_version,
        local_address: local_address.into(),
        local_port,
        remote_address: remote_address.into(),
        remote_port,
        state,
        pid,
        process_name: process_name.into(),
    }
}

fn render_port_rows(
    ui: &mut egui::Ui,
    rows: &[&NetworkEndpoint],
    actions: &mut Vec<AppAction>,
    sort_column: &mut SortColumn,
    sort_ascending: &mut bool,
    selected_endpoint: &mut Option<String>,
) {
    const COLUMN_WIDTHS: [f32; 8] = [70.0, 160.0, 88.0, 160.0, 88.0, 150.0, 78.0, 180.0];
    const HEADER_HEIGHT: f32 = 28.0;
    const ROW_HEIGHT: f32 = 32.0;
    let item_spacing = ui.spacing().item_spacing;
    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
    let table_width = ui.available_width();
    let header = allocate_table_row(ui, table_width, HEADER_HEIGHT, egui::Sense::hover());
    let palette = ui::palette_for_ui(ui);
    header.painter.rect_filled(
        header.response.rect,
        egui::CornerRadius::ZERO,
        ui.visuals().faint_bg_color,
    );
    paint_table_grid(
        &header.painter,
        header.response.rect,
        &COLUMN_WIDTHS,
        palette.border,
    );
    let header_labels = [
        "协议",
        "本地地址",
        "本地端口",
        "远端地址",
        "远端端口",
        "状态",
        "PID",
        "进程",
    ];
    for (index, label) in header_labels.iter().enumerate() {
        let column = match index {
            0 => SortColumn::Protocol,
            1 => SortColumn::LocalAddress,
            2 => SortColumn::LocalAddress,
            3 | 4 => SortColumn::RemoteAddress,
            5 => SortColumn::State,
            6 => SortColumn::Pid,
            7 => SortColumn::Process,
            _ => unreachable!(),
        };
        let cell_rect = table_cell_rect(header.response.rect, &COLUMN_WIDTHS, index);
        let response = ui.interact(
            cell_rect,
            ui.id().with(("port-sort-header", index)),
            egui::Sense::click(),
        );
        response.clone().on_hover_text(format!("按{label}排序"));
        if response.clicked() {
            response.request_focus();
        }
        if response.has_focus() {
            header.painter.rect_stroke(
                cell_rect.shrink(2.0),
                egui::CornerRadius::same(4),
                egui::Stroke::new(1.5_f32, ui::accent(ui)),
                egui::StrokeKind::Middle,
            );
        }
        let activated = response.clicked()
            || (response.has_focus()
                && ui.input(|input| {
                    input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
                }));
        if activated {
            if *sort_column == column {
                *sort_ascending = !*sort_ascending;
            } else {
                *sort_column = column;
                *sort_ascending = true;
            }
        }
        let marker = if *sort_column == column {
            if *sort_ascending { " ▲" } else { " ▼" }
        } else {
            " ↕"
        };
        let header_text_color = if *sort_column == column {
            ui::accent(ui)
        } else {
            ui.visuals().text_color()
        };
        header
            .painter
            .with_clip_rect(cell_rect.shrink2(egui::vec2(6.0, 2.0)))
            .text(
                cell_rect.left_center() + egui::vec2(8.0, 0.0),
                egui::Align2::LEFT_CENTER,
                format!("{label}{marker}"),
                egui::FontId::proportional(13.5),
                header_text_color,
            );
    }
    for (index, endpoint) in rows.iter().enumerate() {
        let row = allocate_table_row(ui, table_width, ROW_HEIGHT, egui::Sense::click());
        let endpoint_key = endpoint_report_line(endpoint);
        let selected = selected_endpoint.as_ref() == Some(&endpoint_key);
        let row_fill = if selected {
            palette.accent.gamma_multiply(0.16)
        } else if row.response.hovered() {
            palette.hover
        } else {
            Color32::TRANSPARENT
        };
        row.painter
            .rect_filled(row.response.rect, egui::CornerRadius::ZERO, row_fill);
        paint_table_grid(
            &row.painter,
            row.response.rect,
            &COLUMN_WIDTHS,
            palette.border,
        );
        if row.response.clicked() {
            *selected_endpoint = Some(endpoint_key);
        }
        let process_name = if endpoint.process_name.is_empty() {
            "进程已退出"
        } else {
            &endpoint.process_name
        };
        let proto_cell = endpoint.protocol.label();
        let pid_cell = endpoint.pid.to_string();

        // 协议
        paint_single_cell(
            &row.painter,
            table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 0),
            proto_cell,
            egui::FontId::monospace(12.5),
            ui.visuals().text_color(),
        );
        ui::show_clipped_text_tooltip(
            ui,
            table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 0),
            ("port-cell", index, 0),
            proto_cell,
            egui::FontId::monospace(12.5),
            (table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 0).width() - 16.0).max(0.0),
        );
        // 本地端点
        paint_single_cell(
            &row.painter,
            table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 1),
            &endpoint.local_address,
            egui::FontId::monospace(13.0),
            ui.visuals().text_color(),
        );
        ui::show_clipped_text_tooltip(
            ui,
            table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 1),
            ("port-cell", index, 1),
            &endpoint.local_address,
            egui::FontId::monospace(13.0),
            (table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 1).width() - 16.0).max(0.0),
        );
        let local_port = endpoint.local_port.to_string();
        paint_single_cell(
            &row.painter,
            table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 2),
            &local_port,
            egui::FontId::monospace(13.0),
            ui.visuals().text_color(),
        );
        ui::show_clipped_text_tooltip(
            ui,
            table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 2),
            ("port-cell", index, 2),
            &local_port,
            egui::FontId::monospace(13.0),
            (table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 2).width() - 16.0).max(0.0),
        );
        let remote_address = if endpoint.remote_address.is_empty() {
            "-"
        } else {
            &endpoint.remote_address
        };
        paint_single_cell(
            &row.painter,
            table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 3),
            remote_address,
            egui::FontId::monospace(13.0),
            if remote_address == "-" {
                ui.visuals().weak_text_color()
            } else {
                ui.visuals().text_color()
            },
        );
        ui::show_clipped_text_tooltip(
            ui,
            table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 3),
            ("port-cell", index, 3),
            remote_address,
            egui::FontId::monospace(13.0),
            (table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 3).width() - 16.0).max(0.0),
        );
        let remote_port = endpoint.remote_port.unwrap_or(0).to_string();
        paint_single_cell(
            &row.painter,
            table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 4),
            &remote_port,
            egui::FontId::monospace(13.0),
            ui.visuals().weak_text_color(),
        );
        ui::show_clipped_text_tooltip(
            ui,
            table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 4),
            ("port-cell", index, 4),
            &remote_port,
            egui::FontId::monospace(13.0),
            (table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 4).width() - 16.0).max(0.0),
        );

        // 状态
        let state_rect = table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 5);
        let state_text = endpoint.state.map_or("-", TcpState::label);
        paint_single_cell(
            &row.painter,
            state_rect,
            state_text,
            egui::FontId::monospace(13.0),
            if state_text == "-" {
                palette.weak
            } else {
                palette.text
            },
        );
        ui::show_clipped_text_tooltip(
            ui,
            state_rect,
            ("port-cell", index, 5),
            state_text,
            egui::FontId::monospace(13.0),
            (table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 5).width() - 16.0).max(0.0),
        );

        // PID
        paint_single_cell(
            &row.painter,
            table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 6),
            &pid_cell,
            egui::FontId::monospace(13.0),
            palette.weak,
        );
        ui::show_clipped_text_tooltip(
            ui,
            table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 6),
            ("port-cell", index, 6),
            &pid_cell,
            egui::FontId::monospace(13.0),
            (table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 6).width() - 16.0).max(0.0),
        );

        // 进程名链接
        let process_rect = table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 7);
        let process_response = ui.interact(
            process_rect,
            ui.id().with(("port-process", index, endpoint.pid)),
            egui::Sense::click(),
        );
        process_response.clone().on_hover_text(format!(
            "点击查看进程详情：{process_name}（PID {}）",
            endpoint.pid
        ));
        let process_color = if process_response.hovered() {
            palette.accent_hover
        } else {
            palette.text
        };
        if process_response.has_focus() {
            row.painter.rect_stroke(
                process_rect.shrink(2.0),
                egui::CornerRadius::same(4),
                egui::Stroke::new(1.5_f32, ui::accent(ui)),
                egui::StrokeKind::Middle,
            );
        }
        let process_painter = row
            .painter
            .with_clip_rect(process_rect.shrink2(egui::vec2(6.0, 2.0)));
        process_painter.text(
            process_rect.left_center() + egui::vec2(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            process_name,
            egui::FontId::proportional(13.5),
            process_color,
        );
        ui::show_clipped_text_tooltip(
            ui,
            process_rect,
            ("port-cell", index, 7),
            process_name,
            egui::FontId::proportional(13.5),
            (table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 7).width() - 16.0).max(0.0),
        );
        if process_response.clicked() {
            process_response.request_focus();
        }
        if process_response.clicked()
            || (process_response.has_focus()
                && ui.input(|input| {
                    input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
                }))
        {
            actions.push(AppAction::InvokeTool(ToolInvocation::process(endpoint.pid)));
        }
    }
    ui.spacing_mut().item_spacing = item_spacing;
}

fn paint_single_cell(
    painter: &egui::Painter,
    rect: egui::Rect,
    text: &str,
    font: egui::FontId,
    color: Color32,
) {
    painter
        .with_clip_rect(rect.shrink2(egui::vec2(6.0, 2.0)))
        .text(
            rect.left_center() + egui::vec2(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            text,
            font,
            color,
        );
}

fn render_selected_endpoint(
    ui: &mut egui::Ui,
    endpoint: &NetworkEndpoint,
    process: Option<&Result<ProcessSummary, AppError>>,
    actions: &mut Vec<AppAction>,
) {
    let palette = ui::palette_for_ui(ui);
    ui::card(ui, |ui| {
        if ui.available_width() >= 720.0 {
            ui.horizontal(|ui| {
                render_selected_endpoint_identity(ui, endpoint, palette.success_text);
                let action_width = 112.0;
                ui.add_space((ui.available_width() - action_width).max(0.0));
                if ui::secondary_button(ui, "跳转到进程").clicked() {
                    actions.push(AppAction::InvokeTool(ToolInvocation::process(endpoint.pid)));
                }
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                render_selected_endpoint_identity(ui, endpoint, palette.success_text);
            });
            ui.add_space(ui::SPACE_8);
            ui.horizontal(|ui| {
                if ui::secondary_button(ui, "跳转到进程").clicked() {
                    actions.push(AppAction::InvokeTool(ToolInvocation::process(endpoint.pid)));
                }
            });
        }
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("PID {}", endpoint.pid));
            ui.separator();
            ui.label(format!("进程 {}", endpoint.process_name));
            if let Some(Ok(process)) = process {
                ui.separator();
                ui.label("路径");
                ui.label(
                    RichText::new(process.exe_path.as_deref().unwrap_or("无法读取")).monospace(),
                );
                ui.separator();
                ui.label("启动时间");
                ui.label(
                    RichText::new(process.started_at.as_deref().unwrap_or("无法读取"))
                        .monospace(),
                );
            }
        });
    });
}

/// 绘制所选端点的摘要字段；宽窄布局共用这一段，避免状态文字与端点格式分叉。
fn render_selected_endpoint_identity(
    ui: &mut egui::Ui,
    endpoint: &NetworkEndpoint,
    status_color: Color32,
) {
    ui.label("已选择");
    ui.label(
        RichText::new(format!(
            "{} {} <-> {}",
            endpoint.protocol.label(),
            endpoint.local_display(),
            endpoint.remote_display()
        ))
        .monospace(),
    );
    if let Some(state) = endpoint.state {
        ui.label(RichText::new(state.label()).color(status_color));
    }
}

struct TableRow {
    response: egui::Response,
    painter: egui::Painter,
}

fn allocate_table_row(ui: &mut egui::Ui, width: f32, height: f32, sense: egui::Sense) -> TableRow {
    let (response, painter) = ui.allocate_painter(egui::vec2(width, height), sense);
    TableRow { response, painter }
}

fn table_cell_rect(row: egui::Rect, column_widths: &[f32], index: usize) -> egui::Rect {
    let declared_width = column_widths.iter().sum::<f32>();
    let scale = row.width() / declared_width;
    let left = row.left() + column_widths[..index].iter().sum::<f32>() * scale;
    egui::Rect::from_min_size(
        egui::pos2(left, row.top()),
        egui::vec2(column_widths[index] * scale, row.height()),
    )
}

/// 绘制表头和数据行的网格线，使列边界在滚动和缩放后仍与数据区域对齐。
fn paint_table_grid(
    painter: &egui::Painter,
    row: egui::Rect,
    column_widths: &[f32],
    color: Color32,
) {
    let stroke = egui::Stroke::new(1.0_f32, color.gamma_multiply(0.85));
    painter.line_segment([row.left_bottom(), row.right_bottom()], stroke);
    for index in 0..column_widths.len().saturating_sub(1) {
        let divider_x = table_cell_rect(row, column_widths, index).right();
        painter.line_segment(
            [
                egui::pos2(divider_x, row.top()),
                egui::pos2(divider_x, row.bottom()),
            ],
            stroke,
        );
    }
}

fn sort_endpoints(rows: &mut [&NetworkEndpoint], column: SortColumn, ascending: bool) {
    rows.sort_by(|left, right| {
        let order = match column {
            SortColumn::None => Ordering::Equal,
            SortColumn::Protocol => left
                .protocol
                .label()
                .cmp(right.protocol.label())
                .then_with(|| left.ip_version.label().cmp(right.ip_version.label())),
            SortColumn::LocalAddress => left
                .local_address
                .cmp(&right.local_address)
                .then_with(|| left.local_port.cmp(&right.local_port)),
            SortColumn::RemoteAddress => left
                .remote_address
                .cmp(&right.remote_address)
                .then_with(|| left.remote_port.cmp(&right.remote_port)),
            SortColumn::State => endpoint_state_label(left).cmp(endpoint_state_label(right)),
            SortColumn::Pid => left.pid.cmp(&right.pid),
            SortColumn::Process => left
                .process_name
                .to_lowercase()
                .cmp(&right.process_name.to_lowercase()),
        };
        if ascending {
            order
        } else {
            reverse_order(order)
        }
    });
}

fn reverse_order(order: Ordering) -> Ordering {
    match order {
        Ordering::Less => Ordering::Greater,
        Ordering::Equal => Ordering::Equal,
        Ordering::Greater => Ordering::Less,
    }
}

fn endpoint_state_label(endpoint: &NetworkEndpoint) -> &'static str {
    endpoint.state.map_or("-", TcpState::label)
}

fn endpoint_report_line(endpoint: &NetworkEndpoint) -> String {
    format!(
        "{} {},{},{},{},{},{}",
        endpoint.protocol.label(),
        endpoint.ip_version.label(),
        endpoint.local_display(),
        endpoint.remote_display(),
        endpoint_state_label(endpoint),
        endpoint.pid,
        if endpoint.process_name.is_empty() {
            "进程已退出"
        } else {
            &endpoint.process_name
        }
    )
}

fn endpoints_csv(rows: &[&NetworkEndpoint]) -> String {
    let mut csv = String::from("协议,IP版本,本地端点,远端端点,状态,PID,进程\r\n");
    for endpoint in rows {
        let values = [
            endpoint.protocol.label().to_owned(),
            endpoint.ip_version.label().to_owned(),
            endpoint.local_display(),
            endpoint.remote_display(),
            endpoint_state_label(endpoint).to_owned(),
            endpoint.pid.to_string(),
            if endpoint.process_name.is_empty() {
                "进程已退出".to_owned()
            } else {
                endpoint.process_name.clone()
            },
        ];
        csv.push_str(
            &values
                .iter()
                .map(|value| csv_cell(value))
                .collect::<Vec<_>>()
                .join(","),
        );
        csv.push_str("\r\n");
    }
    csv
}

fn csv_cell(value: &str) -> String {
    let value = if value.starts_with(['=', '+', '-', '@']) {
        format!("'{value}")
    } else {
        value.to_owned()
    };
    if value.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use crate::model::{IpVersion, NetworkEndpoint, NetworkProtocol, TcpState};

    use super::{PortInspectorTool, SortColumn, csv_cell, endpoints_csv, sort_endpoints};

    fn endpoint(port: u16, pid: u32, name: &str) -> NetworkEndpoint {
        NetworkEndpoint {
            protocol: NetworkProtocol::Tcp,
            ip_version: IpVersion::V4,
            local_address: "127.0.0.1".into(),
            local_port: port,
            remote_address: String::new(),
            remote_port: None,
            state: Some(TcpState::Listen),
            pid,
            process_name: name.into(),
        }
    }

    #[test]
    fn sorts_rows_by_selected_column_and_direction() {
        let values = [endpoint(443, 2, "b.exe"), endpoint(80, 1, "a.exe")];
        let mut rows = values.iter().collect::<Vec<_>>();
        sort_endpoints(&mut rows, SortColumn::LocalAddress, true);
        assert_eq!(
            rows.iter().map(|row| row.local_port).collect::<Vec<_>>(),
            [80, 443]
        );
        sort_endpoints(&mut rows, SortColumn::Pid, false);
        assert_eq!(rows.iter().map(|row| row.pid).collect::<Vec<_>>(), [2, 1]);
    }

    #[test]
    fn csv_escapes_process_names() {
        let values = [endpoint(80, 1, "a,\"b.exe")];
        let rows = values.iter().collect::<Vec<_>>();
        let csv = endpoints_csv(&rows);
        assert!(csv.contains("\"a,\"\"b.exe\""));
    }

    #[test]
    fn csv_cells_escape_formula_prefixes_before_quoting() {
        assert_eq!(csv_cell("=SUM(A1:A2)"), "'=SUM(A1:A2)");
        assert_eq!(csv_cell("@name,tool"), "\"'@name,tool\"");
    }

    #[test]
    fn review_data_matches_port_page_reference_summary() {
        let mut tool = PortInspectorTool::default();
        tool.seed_review();

        assert_eq!(tool.endpoints.len(), 36);
        assert_eq!(
            tool.endpoints
                .iter()
                .filter(|endpoint| endpoint.state == Some(TcpState::Listen))
                .count(),
            18
        );
        assert_eq!(
            tool.endpoints
                .iter()
                .filter(|endpoint| endpoint.state == Some(TcpState::Established))
                .count(),
            6
        );
        assert_eq!(
            tool.endpoints
                .iter()
                .filter(|endpoint| endpoint.protocol == NetworkProtocol::Tcp)
                .count(),
            24
        );
        assert_eq!(
            tool.endpoints
                .iter()
                .filter(|endpoint| endpoint.protocol == NetworkProtocol::Udp)
                .count(),
            12
        );
        assert_eq!(tool.endpoints[1].local_port, 8080);
        assert_eq!(tool.endpoints[1].pid, 5678);
        assert_eq!(tool.selected_process_pid, Some(5678));
    }
}
