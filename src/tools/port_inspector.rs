//! 端口占用工具模块。
//! 自动刷新仅在上一次请求完成后创建下一次后台任务，避免高频查询堆积到 UI 消息队列。

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText, TextEdit};

use crate::{
    core::{
        actions::AppAction,
        invocation::{ToolInvocation, ToolPayload},
        worker::TaskResult,
    },
    model::{AppError, NetworkEndpoint, NetworkProtocol, TcpState},
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

#[derive(Default)]
pub struct PortInspectorTool {
    filter: String,
    protocol: Option<NetworkProtocol>,
    only_listening: bool,
    interval: RefreshInterval,
    endpoints: Vec<NetworkEndpoint>,
    error: Option<AppError>,
    busy: bool,
    last_refresh: Option<Instant>,
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
                    [220.0, 30.0],
                    TextEdit::singleline(&mut self.filter).hint_text("搜索端口、进程或 PID"),
                );
                ui.separator();
                ui.selectable_value(&mut self.protocol, None, "全部协议");
                ui.selectable_value(&mut self.protocol, Some(NetworkProtocol::Tcp), "TCP");
                ui.selectable_value(&mut self.protocol, Some(NetworkProtocol::Udp), "UDP");
                ui.checkbox(&mut self.only_listening, "仅监听");
                egui::ComboBox::from_id_salt("port-refresh")
                    .selected_text(format!("自动刷新：{}", self.interval.label()))
                    .show_ui(ui, |ui| {
                        for interval in RefreshInterval::ALL {
                            ui.selectable_value(&mut self.interval, interval, interval.label());
                        }
                    });
                if ui
                    .add_enabled_ui(!self.busy, |ui| ui::primary_button(ui, "刷新"))
                    .inner
                    .clicked()
                {
                    actions.push(AppAction::RefreshPorts);
                }
            });
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
                self.filter = port.to_string();
                vec![AppAction::RefreshPorts]
            }
            _ => Vec::new(),
        }
    }

    fn handle_task_result(&mut self, result: TaskResult) {
        if let TaskResult::Ports(result) = result {
            self.busy = false;
            self.last_refresh = Some(Instant::now());
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

    fn is_busy(&self) -> bool {
        self.busy
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

    fn render_table(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let filter = self.filter.trim().to_lowercase();
        let rows: Vec<_> = self
            .endpoints
            .iter()
            .filter(|endpoint| {
                self.protocol
                    .is_none_or(|protocol| endpoint.protocol == protocol)
            })
            .filter(|endpoint| !self.only_listening || endpoint.state == Some(TcpState::Listen))
            .filter(|endpoint| endpoint_matches(endpoint, &filter))
            .collect();

        ui.label(
            RichText::new(format!("{} 个端点", rows.len())).color(ui.visuals().weak_text_color()),
        );
        ui.add_space(ui::SPACE_8);
        ui::card(ui, |ui| {
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| render_port_rows(ui, &rows, actions));
        });
    }
}

fn render_port_rows(ui: &mut egui::Ui, rows: &[&NetworkEndpoint], actions: &mut Vec<AppAction>) {
    const COLUMN_WIDTHS: [f32; 6] = [92.0, 290.0, 290.0, 92.0, 84.0, 220.0];
    const ROW_HEIGHT: f32 = 34.0;
    let table_width: f32 = COLUMN_WIDTHS.iter().sum();
    let header = allocate_table_row(ui, table_width, ROW_HEIGHT, egui::Sense::hover());
    header.painter.rect_filled(
        header.response.rect,
        egui::CornerRadius::same(6),
        ui.visuals().faint_bg_color,
    );
    paint_table_cells(
        &header.painter,
        header.response.rect,
        &COLUMN_WIDTHS,
        ["协议", "本地", "远端", "状态", "PID", "进程"],
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
