//! 端口占用工具模块。
//! 自动刷新仅在上一次请求完成后创建下一次后台任务，避免高频查询堆积到 UI 消息队列。

use std::time::{Duration, Instant};

use eframe::egui::{self, RichText, TextEdit};

use crate::{
    core::{actions::AppAction, invocation::ToolPayload, worker::TaskResult},
    model::{AppError, NetworkEndpoint, NetworkProtocol, TcpState},
    tools::{
        ToolModule, ToolUiContext,
        file_lock::{empty_state, error_state, heading},
        registry::{ToolCategory, ToolDescriptor},
    },
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
            icon: "N",
            keywords: &[
                "port", "tcp", "udp", "listen", "socket", "network", "端口", "占用", "网络",
            ],
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _context: ToolUiContext) -> Vec<AppAction> {
        let mut actions = Vec::new();
        heading(ui, "端口占用", "实时查看 TCP / UDP 端点和关联进程。");
        ui.add_space(12.0);
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
                .add_enabled(!self.busy, egui::Button::new("刷新"))
                .clicked()
            {
                actions.push(AppAction::RefreshPorts);
            }
        });
        ui.add_space(12.0);

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
        ui.add_space(5.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("port-table")
                    .striped(true)
                    .min_col_width(84.0)
                    .show(ui, |ui| {
                        ui.strong("协议");
                        ui.strong("本地");
                        ui.strong("远端");
                        ui.strong("状态");
                        ui.strong("PID");
                        ui.strong("进程");
                        ui.end_row();
                        for endpoint in rows {
                            ui.label(format!(
                                "{} {}",
                                endpoint.protocol.label(),
                                endpoint.ip_version.label()
                            ));
                            ui.monospace(endpoint.local_display());
                            ui.monospace(endpoint.remote_display());
                            ui.label(endpoint.state.map(TcpState::label).unwrap_or("-"));
                            ui.monospace(endpoint.pid.to_string());
                            if ui
                                .link(if endpoint.process_name.is_empty() {
                                    "进程已退出"
                                } else {
                                    &endpoint.process_name
                                })
                                .clicked()
                            {
                                actions.push(AppAction::InspectProcess { pid: endpoint.pid });
                            }
                            ui.end_row();
                        }
                    });
            });
    }
}

fn endpoint_matches(endpoint: &NetworkEndpoint, filter: &str) -> bool {
    filter.is_empty()
        || endpoint.local_port.to_string().contains(filter)
        || endpoint.pid.to_string().contains(filter)
        || endpoint.process_name.to_lowercase().contains(filter)
        || endpoint.local_address.to_lowercase().contains(filter)
        || endpoint.remote_address.to_lowercase().contains(filter)
}
