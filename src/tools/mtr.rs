//! 原生 Windows ICMP MTR 路径诊断工具页面。

use std::time::Instant;

use eframe::egui::{self, RichText};

use crate::{
    core::{actions::AppAction, invocation::ToolPayload, worker::TaskResult},
    model::{MtrConfig, MtrHopStats, MtrProgress, MtrResult, PingAddressFamily},
    tools::{
        ToolModule, ToolUiContext,
        registry::{ToolCategory, ToolDescriptor, ToolIcon},
    },
    ui,
};

const RESULT_HEIGHT_RATIO: f32 = 0.60;
const MIN_RESULT_HEIGHT: f32 = 360.0;
const MAX_RESULT_HEIGHT: f32 = 720.0;

#[derive(Default)]
pub struct MtrTool {
    host: String,
    config: MtrConfig,
    busy: bool,
    progress: Option<MtrProgress>,
    result: Option<Result<MtrResult, crate::model::AppError>>,
    validation_error: Option<String>,
}

impl ToolModule for MtrTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: "mtr",
            name: "MTR 路径诊断",
            description: "逐跳观察网络路径、延迟和丢包变化",
            category: ToolCategory::Network,
            icon: ToolIcon::Mtr,
            keywords: &["mtr", "traceroute", "路径", "路由", "ICMP"],
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _context: ToolUiContext) -> Vec<AppAction> {
        let mut actions = Vec::new();
        let palette = ui::palette_for_ui(ui);
        ui::page_heading(ui, "MTR 路由追踪", "逐跳观察网络路径、时延与丢包");
        ui.add_space(ui::SPACE_16);

        ui::card(ui, |ui| {
            ui.horizontal(|ui| {
                let input_width = (ui.available_width() - 320.0).max(220.0);
                ui.add_sized(
                    [input_width, ui::CONTROL_HEIGHT],
                    ui::text_input(&mut self.host, "输入域名或 IP"),
                );
                egui::ComboBox::from_id_salt("mtr-family")
                    .selected_text(self.config.family.label())
                    .show_ui(ui, |ui| {
                        for family in [
                            PingAddressFamily::Auto,
                            PingAddressFamily::V4,
                            PingAddressFamily::V6,
                        ] {
                            ui.selectable_value(&mut self.config.family, family, family.label());
                        }
                    });
                if self.busy {
                    if ui::secondary_button(ui, "停止").clicked() {
                        actions.push(AppAction::StopMtr);
                    }
                } else if ui::primary_button(ui, "开始").clicked() {
                    match self.config.validate() {
                        Ok(()) if !self.host.trim().is_empty() => {
                            self.validation_error = None;
                            actions.push(AppAction::RunMtr {
                                host: self.host.trim().to_owned(),
                                config: self.config.clone(),
                            });
                        }
                        Ok(()) => self.validation_error = Some("目标主机不能为空。".into()),
                        Err(error) => self.validation_error = Some(error),
                    }
                }
            });
            if let Some(error) = &self.validation_error {
                ui.add_space(ui::SPACE_4);
                ui.colored_label(palette.danger_text, error);
            }
        });

        ui.add_space(ui::SPACE_12);
        ui::card(ui, |ui| {
            egui::CollapsingHeader::new(RichText::new("高级参数").strong().color(palette.text))
                .id_salt("mtr-advanced-parameters")
                .default_open(false)
                .show(ui, |ui| {
                    ui.add_space(ui::SPACE_8);
                    egui::Grid::new("mtr-advanced-grid")
                        .num_columns(4)
                        .spacing([16.0, 8.0])
                        .show(ui, |ui| {
                            ui.label("最大跳数");
                            ui.add(egui::DragValue::new(&mut self.config.max_hops).range(1..=64));
                            ui.label("每跳探测");
                            ui.add(
                                egui::DragValue::new(&mut self.config.probes_per_hop).range(1..=10),
                            );
                            ui.end_row();
                            ui.label("单次超时 (ms)");
                            ui.add(
                                egui::DragValue::new(&mut self.config.timeout_ms)
                                    .range(100..=5_000),
                            );
                            ui.label("探测间隔 (ms)");
                            ui.add(
                                egui::DragValue::new(&mut self.config.interval_ms)
                                    .range(200..=5_000),
                            );
                            ui.end_row();
                            ui.label("ICMP 载荷 (bytes)");
                            ui.add(
                                egui::DragValue::new(&mut self.config.payload_size)
                                    .range(0..=1_472),
                            );
                            ui.label("最大并发");
                            ui.add(egui::DragValue::new(&mut self.config.concurrency).range(1..=4));
                            ui.end_row();
                        });
                    ui.horizontal_wrapped(|ui| {
                        let mut finite = self.config.total_rounds.is_some();
                        if ui.checkbox(&mut finite, "有限轮数").changed() {
                            self.config.total_rounds = finite.then_some(10);
                        }
                        if let Some(rounds) = &mut self.config.total_rounds {
                            ui.add(egui::DragValue::new(rounds).range(1..=100));
                        } else {
                            ui.label("持续运行，直到手动停止");
                        }
                        ui.checkbox(&mut self.config.resolve_hostnames, "异步解析跳点主机名");
                    });
                });
        });

        ui.add_space(ui::SPACE_12);
        self.render_results(ui);
        actions
    }

    fn handle_invocation(&mut self, payload: ToolPayload) -> Vec<AppAction> {
        if let ToolPayload::Host { host } = payload {
            self.host = host;
            return vec![AppAction::RunMtr {
                host: self.host.trim().to_owned(),
                config: self.config.clone(),
            }];
        }
        Vec::new()
    }

    fn handle_task_result(&mut self, result: TaskResult) {
        if let TaskResult::Mtr(result) = result {
            self.busy = false;
            if let Ok(value) = &result {
                self.progress = Some(MtrProgress {
                    host: value.host.clone(),
                    round: value.rounds,
                    hops: value.hops.clone(),
                });
            }
            self.result = Some(result);
        }
    }

    fn handle_mtr_progress(&mut self, progress: MtrProgress) {
        self.progress = Some(progress);
    }

    fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
        if busy {
            self.result = None;
            self.progress = None;
        }
    }

    fn poll_actions(&mut self, _now: Instant) -> Vec<AppAction> {
        Vec::new()
    }
}

impl MtrTool {
    fn render_results(&self, ui: &mut egui::Ui) {
        let palette = ui::palette_for_ui(ui);
        let Some(progress) = &self.progress else {
            if let Some(Err(error)) = &self.result {
                let (title, detail) = error.user_message();
                ui::state_card(ui, title, &detail, palette.danger_text);
            } else if self.busy {
                ui::state_card(
                    ui,
                    "正在发起路径探测",
                    "正在解析目标主机并发送 ICMP TTL 递增探测包...",
                    palette.accent,
                );
            } else if !self.busy {
                ui::state_card(
                    ui,
                    "等待开始诊断",
                    "输入目标域名或 IP 地址后点击“开始 MTR 诊断”。",
                    palette.accent,
                );
            }
            return;
        };

        let active_hops = progress.hops.iter().filter(|hop| hop.sent > 0).count();
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0_f32, palette.border_subtle))
            .corner_radius(egui::CornerRadius::same(5))
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui::status_pill(
                        ui,
                        if self.busy { "运行中" } else { "已停止" },
                        if self.busy {
                            palette.success_text
                        } else {
                            palette.weak
                        },
                    );
                    ui.separator();
                    ui.label(format!("轮次：{}", progress.round));
                    ui.separator();
                    ui.label(format!("已探测：{active_hops} 跳"));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(&progress.host)
                                .monospace()
                                .color(palette.weak),
                        );
                    });
                });
            });
        ui.add_space(ui::SPACE_12);

        let result_height = result_table_height(ui.ctx().screen_rect().height());
        ui::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("逐跳链路拓扑与时延表 ({})", progress.host))
                        .strong()
                        .size(14.5)
                        .color(palette.text),
                );
                if self.busy {
                    ui.add_space(ui::SPACE_4);
                    ui::status_pill(ui, "PROBING", palette.success_text);
                }
            });
            ui.add_space(ui::SPACE_8);
            egui::ScrollArea::both()
                .id_salt("mtr-results-scroll")
                .max_height(result_height)
                .min_scrolled_height(result_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_min_height(result_height);
                    egui::Grid::new("mtr-result-grid")
                        .striped(true)
                        .min_col_width(70.0)
                        .show(ui, |ui| {
                            for header in [
                                "跳数", "地址", "发送", "接收", "丢包", "最近", "平均", "最小",
                                "最大", "状态",
                            ] {
                                ui.label(RichText::new(header).strong().color(palette.text));
                            }
                            ui.end_row();
                            for hop in progress.hops.iter().filter(|hop| hop.sent > 0) {
                                render_hop(ui, hop);
                                ui.end_row();
                            }
                        });
                });
        });
        if let Some(Ok(result)) = &self.result {
            ui.add_space(ui::SPACE_8);
            let state = if result.stopped {
                "已停止"
            } else {
                "已完成"
            };
            ui.label(
                RichText::new(format!("{state}，完成 {} 轮探测", result.rounds))
                    .color(palette.success_text),
            );
        }
    }
}

fn result_table_height(viewport_height: f32) -> f32 {
    (viewport_height * RESULT_HEIGHT_RATIO).clamp(MIN_RESULT_HEIGHT, MAX_RESULT_HEIGHT)
}

fn render_hop(ui: &mut egui::Ui, hop: &MtrHopStats) {
    ui.label(hop.hop.to_string());
    let address = hop.address.as_deref().unwrap_or("*").to_owned();
    let text = hop.hostname.as_deref().map_or(address.clone(), |hostname| {
        format!("{address} ({hostname})")
    });
    ui.label(text).on_hover_text(address);
    ui.label(hop.sent.to_string());
    ui.label(hop.received.to_string());
    ui.label(format!("{:.1}%", hop.loss_percent()));
    ui.label(format_ms(hop.last_ms));
    ui.label(format_ms(hop.avg_ms));
    ui.label(format_ms(hop.min_ms));
    ui.label(format_ms(hop.max_ms));
    ui.label(&hop.status);
}

fn format_ms(value: Option<f64>) -> String {
    value.map_or_else(|| "-".into(), |value| format!("{value:.1} ms"))
}

#[cfg(test)]
mod tests {
    use super::{MAX_RESULT_HEIGHT, MIN_RESULT_HEIGHT, result_table_height};

    #[test]
    fn result_table_height_uses_viewport_ratio_with_bounds() {
        assert_eq!(result_table_height(500.0), MIN_RESULT_HEIGHT);
        assert_eq!(result_table_height(1_000.0), 600.0);
        assert_eq!(result_table_height(2_000.0), MAX_RESULT_HEIGHT);
    }
}
