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
    model::{AppError, IpVersion, NetworkEndpoint, NetworkProtocol, TcpState},
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
}

impl RefreshInterval {
    const ALL: [Self; 4] = [
        Self::Off,
        Self::OneSecond,
        Self::TwoSeconds,
        Self::FiveSeconds,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Off => "关闭",
            Self::OneSecond => "1 秒",
            Self::TwoSeconds => "2 秒",
            Self::FiveSeconds => "5 秒",
        }
    }

    fn duration(self) -> Option<Duration> {
        match self {
            Self::Off => None,
            Self::OneSecond => Some(Duration::from_secs(1)),
            Self::TwoSeconds => Some(Duration::from_secs(2)),
            Self::FiveSeconds => Some(Duration::from_secs(5)),
        }
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum SortColumn {
    Protocol,
    LocalAddress,
    RemoteAddress,
    State,
    #[default]
    Pid,
    Process,
}

#[derive(Default)]
pub struct PortInspectorTool {
    filter: String,
    exact_port: String,
    exact_pid: Option<u32>,
    protocol: Option<NetworkProtocol>,
    ip_version: Option<IpVersion>,
    state: Option<TcpState>,
    sort_column: SortColumn,
    sort_ascending: bool,
    interval: RefreshInterval,
    endpoints: Vec<NetworkEndpoint>,
    error: Option<AppError>,
    busy: bool,
    last_refresh: Option<Instant>,
    last_refresh_label: Option<String>,
}

impl ToolModule for PortInspectorTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: "port-inspector",
            name: "端口占用",
            description: "检查 TCP 和 UDP 端口、连接状态与所属进程",
            category: ToolCategory::Network,
            icon: ToolIcon::Network,
            keywords: &[
                "port", "tcp", "udp", "listen", "socket", "network", "端口", "占用", "网络",
            ],
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _context: ToolUiContext) -> Vec<AppAction> {
        let mut actions = Vec::new();
        let palette = ui::palette_for_ui(ui);
        heading(ui, "端口监控", "查看 TCP、UDP 端点、连接状态与所属进程");
        ui.add_space(ui::SPACE_16);

        ui::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.add_sized(
                    [220.0, ui::CONTROL_HEIGHT],
                    ui::text_input(&mut self.filter, "搜索地址、进程或 PID"),
                );
                ui.add_sized(
                    [110.0, ui::CONTROL_HEIGHT],
                    ui::text_input(&mut self.exact_port, "精确端口"),
                );
                ui.separator();
                egui::ComboBox::from_id_salt("port-protocol")
                    .selected_text(self.protocol.map_or("全部协议", NetworkProtocol::label))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.protocol, None, "全部协议");
                        ui.selectable_value(&mut self.protocol, Some(NetworkProtocol::Tcp), "TCP");
                        ui.selectable_value(&mut self.protocol, Some(NetworkProtocol::Udp), "UDP");
                    });
                egui::ComboBox::from_id_salt("port-ip-version")
                    .selected_text(self.ip_version.map_or("全部 IP", IpVersion::label))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.ip_version, None, "全部 IP");
                        ui.selectable_value(&mut self.ip_version, Some(IpVersion::V4), "IPv4");
                        ui.selectable_value(&mut self.ip_version, Some(IpVersion::V6), "IPv6");
                    });
                egui::ComboBox::from_id_salt("port-state")
                    .selected_text(self.state.map_or("全部状态", TcpState::label))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.state, None, "全部状态");
                        for state in TcpState::ALL {
                            ui.selectable_value(&mut self.state, Some(state), state.label());
                        }
                    });
                egui::ComboBox::from_id_salt("port-refresh")
                    .selected_text(format!("自动刷新：{}", self.interval.label()))
                    .show_ui(ui, |ui| {
                        for interval in RefreshInterval::ALL {
                            ui.selectable_value(&mut self.interval, interval, interval.label());
                        }
                    });
                if ui::primary_button(
                    ui,
                    if self.busy {
                        "重新查询"
                    } else {
                        "立即刷新"
                    },
                )
                .clicked()
                {
                    actions.push(AppAction::RefreshPorts);
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
        if let TaskResult::Ports(result) = result {
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
    pub fn due_refresh(&self, now: Instant) -> bool {
        self.interval.duration().is_some_and(|interval| {
            !self.busy
                && self
                    .last_refresh
                    .is_none_or(|last| now.duration_since(last) >= interval)
        })
    }

    fn render_table(&mut self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let palette = ui::palette_for_ui(ui);
        let filter = self.filter.trim().to_lowercase();
        let exact_port = self
            .exact_port
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0);
        let invalid_port = !self.exact_port.trim().is_empty() && exact_port.is_none();
        let mut rows: Vec<_> = self
            .endpoints
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
            .filter(|endpoint| self.exact_pid.is_none_or(|pid| endpoint.pid == pid))
            .filter(|endpoint| endpoint_matches(endpoint, &filter))
            .collect();
        if invalid_port {
            rows.clear();
        }
        sort_endpoints(&mut rows, self.sort_column, self.sort_ascending);

        let listening_count = rows
            .iter()
            .filter(|r| r.state == Some(TcpState::Listen))
            .count();
        let established_count = rows
            .iter()
            .filter(|r| r.state == Some(TcpState::Established))
            .count();

        // 顶部网络端点指标磁贴
        let tile_w = ((ui.available_width() - ui::SPACE_12 * 2.0) / 3.0).max(140.0);
        ui.horizontal_wrapped(|ui| {
            ui::metric_tile(
                ui,
                tile_w,
                "匹配端点总数",
                &rows.len().to_string(),
                "个端点",
                palette.accent,
            );
            ui::metric_tile(
                ui,
                tile_w,
                "监听状态 (LISTEN)",
                &listening_count.to_string(),
                "个端口",
                palette.success_text,
            );
            ui::metric_tile(
                ui,
                tile_w,
                "已连接 (ESTABLISHED)",
                &established_count.to_string(),
                "条活动连接",
                palette.accent_secondary,
            );
        });
        ui.add_space(ui::SPACE_12);

        ui.horizontal(|ui| {
            let refresh = self
                .last_refresh_label
                .as_deref()
                .map_or_else(|| "尚未刷新".to_owned(), |time| format!("更新于 {time}"));
            ui.label(
                RichText::new(format!(
                    "已捕获 {} 条网络套接字记录 · {}",
                    rows.len(),
                    refresh
                ))
                .size(13.0)
                .color(palette.weak),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui::small_action_button(ui, "导出 CSV").clicked() {
                    actions.push(AppAction::ExportPortsCsv {
                        content: endpoints_csv(&rows),
                    });
                }
                if ui::small_action_button(ui, "复制 CSV").clicked() {
                    actions.push(AppAction::CopyText(endpoints_csv(&rows)));
                }
            });
        });
        if invalid_port {
            ui.add_space(ui::SPACE_4);
            ui.label(RichText::new("精确端口必须是 1 到 65535 的整数").color(ui::danger_text(ui)));
        }
        ui.add_space(ui::SPACE_8);
        let table_height = (ui.ctx().screen_rect().height() * 0.58).clamp(420.0, 950.0);
        ui::card(ui, |ui| {
            egui::ScrollArea::both()
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
                        &mut self.sort_column,
                        &mut self.sort_ascending,
                    )
                });
        });
    }
}

fn render_port_rows(
    ui: &mut egui::Ui,
    rows: &[&NetworkEndpoint],
    actions: &mut Vec<AppAction>,
    sort_column: &mut SortColumn,
    sort_ascending: &mut bool,
) {
    const COLUMN_WIDTHS: [f32; 7] = [92.0, 290.0, 290.0, 112.0, 84.0, 220.0, 78.0];
    const ROW_HEIGHT: f32 = 36.0;
    let table_width: f32 = COLUMN_WIDTHS.iter().sum();
    let header = allocate_table_row(ui, table_width, ROW_HEIGHT, egui::Sense::hover());
    let palette = ui::palette_for_ui(ui);
    header.painter.rect_filled(
        header.response.rect,
        egui::CornerRadius::same(6),
        ui.visuals().faint_bg_color,
    );
    let header_labels = [
        "协议",
        "本地端点",
        "远端端点",
        "连接状态",
        "PID",
        "所属进程",
        "操作",
    ];
    for (index, label) in header_labels.iter().enumerate() {
        let column = match index {
            0 => SortColumn::Protocol,
            1 => SortColumn::LocalAddress,
            2 => SortColumn::RemoteAddress,
            3 => SortColumn::State,
            4 => SortColumn::Pid,
            5 => SortColumn::Process,
            _ => continue,
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
            ""
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
    header.painter.with_clip_rect(header.response.rect).text(
        table_cell_rect(header.response.rect, &COLUMN_WIDTHS, 6).left_center()
            + egui::vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        "操作",
        egui::FontId::proportional(13.5),
        ui.visuals().text_color(),
    );

    for (index, endpoint) in rows.iter().enumerate() {
        let row = allocate_table_row(ui, table_width, ROW_HEIGHT, egui::Sense::hover());
        let row_fill = if row.response.hovered() {
            ui.visuals().widgets.hovered.bg_fill
        } else if index % 2 == 1 {
            ui.visuals().faint_bg_color
        } else {
            Color32::TRANSPARENT
        };
        row.painter
            .rect_filled(row.response.rect, egui::CornerRadius::same(4), row_fill);
        let process_name = if endpoint.process_name.is_empty() {
            "进程已退出"
        } else {
            &endpoint.process_name
        };
        let proto_cell = format!(
            "{} {}",
            endpoint.protocol.label(),
            endpoint.ip_version.label()
        );
        let local_cell = endpoint.local_display();
        let remote_cell = endpoint.remote_display();
        let pid_cell = endpoint.pid.to_string();

        // 协议
        paint_single_cell(
            &row.painter,
            table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 0),
            &proto_cell,
            egui::FontId::monospace(12.5),
            ui.visuals().text_color(),
        );
        // 本地端点
        paint_single_cell(
            &row.painter,
            table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 1),
            &local_cell,
            egui::FontId::monospace(13.0),
            ui.visuals().text_color(),
        );
        // 远端端点
        paint_single_cell(
            &row.painter,
            table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 2),
            &remote_cell,
            egui::FontId::monospace(13.0),
            if remote_cell == "-" {
                ui.visuals().weak_text_color()
            } else {
                ui.visuals().text_color()
            },
        );

        // 状态徽标
        let state_rect = table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 3);
        let (state_text, fg_color, bg_color) = match endpoint.state {
            Some(TcpState::Listen) => (
                "LISTEN",
                palette.success_text,
                palette
                    .success_text
                    .gamma_multiply(if ui.visuals().dark_mode { 0.22 } else { 0.12 }),
            ),
            Some(TcpState::Established) => (
                "ESTABLISHED",
                palette.accent,
                palette
                    .accent
                    .gamma_multiply(if ui.visuals().dark_mode { 0.22 } else { 0.12 }),
            ),
            Some(TcpState::CloseWait) | Some(TcpState::TimeWait) => (
                "WAIT",
                palette.warning_text,
                palette
                    .warning_text
                    .gamma_multiply(if ui.visuals().dark_mode { 0.22 } else { 0.12 }),
            ),
            Some(state) => (state.label(), palette.weak, palette.border_subtle),
            None => ("-", palette.weak, Color32::TRANSPARENT),
        };
        if state_text != "-" {
            let chip_rect = egui::Rect::from_center_size(
                state_rect.left_center() + egui::vec2(44.0, 0.0),
                egui::vec2(84.0, 20.0),
            );
            row.painter
                .rect_filled(chip_rect, egui::CornerRadius::same(10), bg_color);
            row.painter.text(
                chip_rect.center(),
                egui::Align2::CENTER_CENTER,
                state_text,
                egui::FontId::proportional(11.5),
                fg_color,
            );
        } else {
            paint_single_cell(
                &row.painter,
                state_rect,
                "-",
                egui::FontId::proportional(13.0),
                palette.weak,
            );
        }

        // PID
        paint_single_cell(
            &row.painter,
            table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 4),
            &pid_cell,
            egui::FontId::monospace(13.0),
            palette.weak,
        );

        // 进程名链接
        let process_rect = table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 5);
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
            ui::accent(ui)
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

        // 操作复制按钮
        let copy_rect = table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 6);
        let copy_button_rect =
            egui::Rect::from_center_size(copy_rect.center(), egui::vec2(48.0, 24.0));
        let copy_response = ui.interact(
            copy_button_rect,
            ui.id().with(("copy-port-row", index, endpoint.pid)),
            egui::Sense::click(),
        );
        copy_response.clone().on_hover_text("复制该端点记录");
        let btn_fill = if copy_response.hovered() {
            palette.secondary_button_hover
        } else {
            palette.secondary_button_bg
        };
        row.painter.rect(
            copy_button_rect,
            egui::CornerRadius::same(4),
            btn_fill,
            egui::Stroke::new(1.0_f32, palette.border_subtle),
            egui::StrokeKind::Middle,
        );
        if copy_response.has_focus() {
            row.painter.rect_stroke(
                copy_button_rect.shrink(1.0),
                egui::CornerRadius::same(4),
                egui::Stroke::new(1.5_f32, ui::accent(ui)),
                egui::StrokeKind::Middle,
            );
        }
        row.painter.text(
            copy_button_rect.center(),
            egui::Align2::CENTER_CENTER,
            "复制",
            egui::FontId::proportional(12.0),
            palette.text,
        );
        if copy_response.clicked()
            || (copy_response.has_focus()
                && ui.input(|input| {
                    input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
                }))
        {
            actions.push(AppAction::CopyText(endpoint_report_line(endpoint)));
        }
    }
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

struct TableRow {
    response: egui::Response,
    painter: egui::Painter,
}

fn allocate_table_row(ui: &mut egui::Ui, width: f32, height: f32, sense: egui::Sense) -> TableRow {
    let (response, painter) = ui.allocate_painter(egui::vec2(width, height), sense);
    TableRow { response, painter }
}

fn table_cell_rect(row: egui::Rect, column_widths: &[f32], index: usize) -> egui::Rect {
    let left = row.left() + column_widths[..index].iter().sum::<f32>();
    egui::Rect::from_min_size(
        egui::pos2(left, row.top()),
        egui::vec2(column_widths[index], row.height()),
    )
}

fn endpoint_matches(endpoint: &NetworkEndpoint, filter: &str) -> bool {
    filter.is_empty()
        || endpoint.local_port.to_string().contains(filter)
        || endpoint.pid.to_string().contains(filter)
        || endpoint.process_name.to_lowercase().contains(filter)
        || endpoint.local_address.to_lowercase().contains(filter)
        || endpoint.remote_address.to_lowercase().contains(filter)
}

fn sort_endpoints(rows: &mut [&NetworkEndpoint], column: SortColumn, ascending: bool) {
    rows.sort_by(|left, right| {
        let order = match column {
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

    use super::{SortColumn, csv_cell, endpoints_csv, sort_endpoints};

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
}
