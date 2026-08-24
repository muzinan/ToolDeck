//! 端口占用工具模块。
//! 自动刷新仅在上一次请求完成后创建下一次后台任务，避免高频查询堆积到 UI 消息队列。

use std::{
    cmp::Ordering,
    time::{Duration, Instant},
};

use eframe::egui::{self, Color32, RichText, TextEdit};

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
        heading(ui, "端口占用", "实时查看 TCP / UDP 端点和关联进程。");
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.add_sized(
                    [210.0, 30.0],
                    TextEdit::singleline(&mut self.filter).hint_text("搜索地址、进程或 PID"),
                );
                ui.add_sized(
                    [118.0, 30.0],
                    TextEdit::singleline(&mut self.exact_port).hint_text("精确端口"),
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
                if ui::primary_button(ui, if self.busy { "再次刷新" } else { "刷新" }).clicked()
                {
                    actions.push(AppAction::RefreshPorts);
                }
            });
            if let Some(pid) = self.exact_pid {
                ui.add_space(ui::SPACE_8);
                ui.horizontal_wrapped(|ui| {
                    ui.label(format!("精确 PID 筛选：{pid}"));
                    if ui.small_button("清除").clicked() {
                        self.exact_pid = None;
                    }
                });
            }
        });
        ui.add_space(ui::SPACE_16);

        if self.busy && self.endpoints.is_empty() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("正在读取系统端口表...");
            });
        }
        if let Some(error) = &self.error {
            error_state(ui, error);
        } else if self.endpoints.is_empty() && !self.busy {
            empty_state(ui, "还没有端口数据", "点击刷新，或保持自动刷新开启。")
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

        ui.horizontal_wrapped(|ui| {
            let refresh = self
                .last_refresh_label
                .as_deref()
                .map_or_else(|| "尚未刷新".to_owned(), |time| format!("刷新于 {time}"));
            ui.label(
                RichText::new(format!("{} 个端点 · {refresh}", rows.len()))
                    .color(ui.visuals().weak_text_color()),
            );
            if ui.small_button("复制 CSV").clicked() {
                actions.push(AppAction::CopyText(endpoints_csv(&rows)));
            }
            if ui.small_button("导出 CSV").clicked() {
                actions.push(AppAction::ExportPortsCsv {
                    content: endpoints_csv(&rows),
                });
            }
        });
        if invalid_port {
            ui.label(RichText::new("精确端口必须是 1 到 65535 的整数").color(ui::danger_text(ui)));
        }
        ui.add_space(ui::SPACE_8);
        ui::card(ui, |ui| {
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
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
    const ROW_HEIGHT: f32 = 34.0;
    let table_width: f32 = COLUMN_WIDTHS.iter().sum();
    let header = allocate_table_row(ui, table_width, ROW_HEIGHT, egui::Sense::hover());
    header.painter.rect_filled(
        header.response.rect,
        egui::CornerRadius::same(6),
        ui.visuals().faint_bg_color,
    );
    let header_labels = ["协议", "本地", "远端", "状态", "PID", "进程", "操作"];
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
            if *sort_ascending { " ↑" } else { " ↓" }
        } else {
            ""
        };
        header
            .painter
            .with_clip_rect(cell_rect.shrink2(egui::vec2(6.0, 2.0)))
            .text(
                cell_rect.left_center() + egui::vec2(8.0, 0.0),
                egui::Align2::LEFT_CENTER,
                format!("{label}{marker}"),
                egui::FontId::proportional(14.0),
                ui.visuals().text_color(),
            );
    }
    header.painter.with_clip_rect(header.response.rect).text(
        table_cell_rect(header.response.rect, &COLUMN_WIDTHS, 6).left_center()
            + egui::vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        "操作",
        egui::FontId::proportional(14.0),
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
        let cells = [
            format!(
                "{} {}",
                endpoint.protocol.label(),
                endpoint.ip_version.label()
            ),
            endpoint.local_display(),
            endpoint.remote_display(),
            endpoint
                .state
                .map(TcpState::label)
                .unwrap_or("-")
                .to_owned(),
            endpoint.pid.to_string(),
            process_name.to_owned(),
        ];
        paint_table_cells(
            &row.painter,
            row.response.rect,
            &COLUMN_WIDTHS[..5],
            cells[..5].iter().map(String::as_str),
            egui::FontId::monospace(13.0),
            ui.visuals().text_color(),
        );
        let process_rect = table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 5);
        let process_response = ui.interact(
            process_rect,
            ui.id().with(("port-process", index, endpoint.pid)),
            egui::Sense::click(),
        );
        process_response
            .clone()
            .on_hover_text(format!("查看进程：{process_name}（PID {}）", endpoint.pid));
        let process_color = if process_response.hovered() {
            ui::accent(ui).gamma_multiply(0.75)
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
            &cells[5],
            egui::FontId::proportional(14.0),
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
        let copy_rect = table_cell_rect(row.response.rect, &COLUMN_WIDTHS, 6);
        let copy_response = ui.interact(
            copy_rect,
            ui.id().with(("copy-port-row", index, endpoint.pid)),
            egui::Sense::click(),
        );
        copy_response.clone().on_hover_text("复制这一行端点信息");
        if copy_response.clicked() {
            copy_response.request_focus();
        }
        if copy_response.has_focus() {
            row.painter.rect_stroke(
                copy_rect.shrink(2.0),
                egui::CornerRadius::same(4),
                egui::Stroke::new(1.5_f32, ui::accent(ui)),
                egui::StrokeKind::Middle,
            );
        }
        row.painter
            .with_clip_rect(copy_rect.shrink2(egui::vec2(6.0, 2.0)))
            .text(
                copy_rect.center(),
                egui::Align2::CENTER_CENTER,
                "复制",
                egui::FontId::proportional(14.0),
                ui::accent(ui),
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

struct TableRow {
    response: egui::Response,
    painter: egui::Painter,
}

fn allocate_table_row(ui: &mut egui::Ui, width: f32, height: f32, sense: egui::Sense) -> TableRow {
    let (response, painter) = ui.allocate_painter(egui::vec2(width, height), sense);
    TableRow { response, painter }
}

fn paint_table_cells<'a>(
    painter: &egui::Painter,
    row: egui::Rect,
    column_widths: &[f32],
    cells: impl IntoIterator<Item = &'a str>,
    font: egui::FontId,
    color: Color32,
) {
    let mut x = row.left();
    for (width, text) in column_widths.iter().zip(cells) {
        let cell_rect =
            egui::Rect::from_min_size(egui::pos2(x, row.top()), egui::vec2(*width, row.height()));
        painter
            .with_clip_rect(cell_rect.shrink2(egui::vec2(6.0, 2.0)))
            .text(
                egui::pos2(x + 8.0, row.center().y),
                egui::Align2::LEFT_CENTER,
                text,
                font.clone(),
                color,
            );
        x += width;
    }
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
