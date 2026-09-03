//! 原生 Windows ICMP MTR 路径诊断工具页面。

use std::{collections::BTreeSet, time::Instant};

use eframe::egui::{self, RichText};

use crate::{
    core::{actions::AppAction, invocation::ToolPayload, worker::TaskResult},
    model::{MtrConfig, MtrHopStats, MtrProgress, MtrResult, PingAddressFamily},
    platform::windows::local_time_hms_millis,
    tools::{
        ToolModule, ToolUiContext,
        registry::{ToolCategory, ToolDescriptor, ToolIcon},
    },
    ui,
};

pub struct MtrTool {
    host: String,
    config: MtrConfig,
    busy: bool,
    progress: Option<MtrProgress>,
    result: Option<Result<MtrResult, crate::model::AppError>>,
    validation_error: Option<String>,
    hop_history: Vec<Vec<Option<f64>>>,
    history_times: Vec<String>,
    trend_limit: usize,
    selected_trend_hops: BTreeSet<u8>,
    selected_hop: Option<u8>,
    trend_selection_ready: bool,
    review_seeded: bool,
}

impl Default for MtrTool {
    fn default() -> Self {
        Self {
            host: String::new(),
            config: MtrConfig::default(),
            busy: false,
            progress: None,
            result: None,
            validation_error: None,
            hop_history: Vec::new(),
            history_times: Vec::new(),
            trend_limit: 7,
            selected_trend_hops: BTreeSet::new(),
            selected_hop: None,
            trend_selection_ready: false,
            review_seeded: false,
        }
    }
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

    fn ui(&mut self, ui: &mut egui::Ui, context: ToolUiContext) -> Vec<AppAction> {
        if context.review_mode && !self.review_seeded {
            self.seed_review();
        }
        let mut actions = Vec::new();
        let palette = ui::palette_for_ui(ui);
        ui::page_heading(ui, "MTR", "按 TTL 持续统计真实路由节点");
        ui.add_space(0.0);

        ui::card(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new("目标主机").size(12.0).color(palette.weak));
                    ui.add_sized(
                        [220.0, ui::CONTROL_HEIGHT],
                        ui::text_input(&mut self.host, "域名或 IP"),
                    );
                });
                mtr_family_control(ui, &mut self.config.family, palette);
                ui.vertical(|ui| {
                    ui.label(RichText::new("模式").size(12.0).color(palette.weak));
                    let finite = self.config.total_rounds.is_some();
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        if ui
                            .add_sized(
                                [72.0, ui::CONTROL_HEIGHT],
                                egui::Button::selectable(finite, "有限轮次"),
                            )
                            .clicked()
                        {
                            self.config.total_rounds = Some(20);
                        }
                        if ui
                            .add_sized(
                                [64.0, ui::CONTROL_HEIGHT],
                                egui::Button::selectable(!finite, "持续"),
                            )
                            .clicked()
                        {
                            self.config.total_rounds = None;
                        }
                    });
                });
                mtr_round_control(ui, &mut self.config.total_rounds, palette);
                mtr_drag_u32(
                    ui,
                    "超时",
                    &mut self.config.timeout_ms,
                    100..=5_000,
                    " ms",
                    palette,
                );
                ui.vertical(|ui| {
                    ui.label(RichText::new("每跳探测").size(12.0).color(palette.weak));
                    ui.add(
                        egui::DragValue::new(&mut self.config.probes_per_hop)
                            .range(1..=10)
                            .min_decimals(0),
                    );
                });
                ui.vertical(|ui| {
                    ui.label(" ");
                    ui.add_enabled_ui(!self.busy, |ui| {
                        if ui::primary_button_sized(ui, "开始", [78.0, ui::CONTROL_HEIGHT])
                            .clicked()
                        {
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
                });
                ui.vertical(|ui| {
                    ui.label(" ");
                    if ui
                        .add_enabled(
                            self.busy,
                            egui::Button::new(
                                RichText::new("停止").strong().color(palette.danger_text),
                            )
                            .min_size(egui::vec2(78.0, ui::CONTROL_HEIGHT)),
                        )
                        .clicked()
                    {
                        actions.push(AppAction::StopMtr);
                    }
                });
            });
            if let Some(error) = &self.validation_error {
                ui.add_space(ui::SPACE_4);
                ui.colored_label(palette.danger_text, error);
            }
            ui.add_space(0.0);
            ui.separator();
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
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.config.resolve_hostnames, "异步解析跳点主机名");
                    });
                });
        });
        ui.add_space(0.0);
        self.render_results(ui, &mut actions);
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
        self.push_history(&progress);
        self.progress = Some(progress);
    }

    fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
        if busy {
            self.result = None;
            self.progress = None;
            self.hop_history.clear();
            self.history_times.clear();
            self.selected_trend_hops.clear();
            self.trend_selection_ready = false;
        }
    }

    fn poll_actions(&mut self, _now: Instant) -> Vec<AppAction> {
        Vec::new()
    }
}

impl MtrTool {
    fn render_results(&mut self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let palette = ui::palette_for_ui(ui);
        let Some(progress) = self.progress.clone() else {
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
        let table_height = (ui.ctx().screen_rect().height() * 0.28).clamp(250.0, 266.0);
        ui::card(ui, |ui| {
            ui.horizontal(|ui| {
                let status_color = if self.busy {
                    palette.success_text
                } else {
                    palette.weak
                };
                let (dot_rect, _) =
                    ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter()
                    .circle_filled(dot_rect.center(), 4.0, status_color);
                ui.label(
                    RichText::new(if self.busy { "运行中" } else { "已停止" }).color(status_color),
                );
                ui.label(RichText::new("·").color(palette.weak));
                ui.label(format!("第 {} 轮", progress.round));
                ui.label(RichText::new("·").color(palette.weak));
                ui.label(format!("已探测 {active_hops} 跳"));
            });
            ui.add_space(ui::SPACE_4);
            let column_widths = mtr_table_column_widths(ui.available_width());
            render_mtr_table_header(ui, &column_widths);
            egui::ScrollArea::both()
                .id_salt("mtr-results-scroll")
                .max_height(table_height)
                .min_scrolled_height(table_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for hop in progress.hops.iter().filter(|hop| hop.sent > 0) {
                        render_mtr_table_row(ui, hop, &column_widths, &mut self.selected_hop);
                    }
                });
        });
        ui.add_space(ui::SPACE_12);
        self.render_trends(ui, &progress, actions);
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

    fn push_history(&mut self, progress: &MtrProgress) {
        self.hop_history.push(
            progress
                .hops
                .iter()
                .map(|hop| hop.last_ms)
                .collect::<Vec<_>>(),
        );
        self.history_times.push(local_time_hms_millis());
        if self.hop_history.len() > 120 {
            self.hop_history.remove(0);
            self.history_times.remove(0);
        }
    }

    fn seed_review(&mut self) {
        self.review_seeded = true;
        self.host = "example.com".into();
        self.config.total_rounds = None;
        self.config.probes_per_hop = 3;
        self.busy = true;
        self.selected_hop = Some(4);
        let rows = [
            ("192.0.2.1", 0.0, 0.271, 0.243, 0.203, 0.318, 0.022),
            ("192.0.2.254", 0.0, 1.372, 1.285, 1.142, 1.632, 0.098),
            ("192.0.2.2", 0.0, 2.831, 2.764, 2.512, 3.121, 0.137),
            ("198.51.100.1", 0.0, 5.217, 5.086, 4.782, 5.612, 0.173),
            ("198.51.100.9", 0.0, 10.685, 10.512, 10.102, 11.048, 0.198),
            ("203.0.113.5", 5.6, 15.432, 15.218, 14.803, 16.021, 0.241),
            ("203.0.113.5", 5.6, 15.498, 15.301, 14.912, 16.087, 0.228),
        ];
        let hops = rows
            .iter()
            .enumerate()
            .map(
                |(index, (address, loss, last, avg, min, max, jitter))| MtrHopStats {
                    hop: index as u8 + 1,
                    address: Some((*address).into()),
                    hostname: Some((*address).into()),
                    sent: 36,
                    received: if *loss > 0.0 { 34 } else { 36 },
                    min_ms: Some(*min),
                    avg_ms: Some(*avg),
                    max_ms: Some(*max),
                    last_ms: Some(*last),
                    jitter_ms: Some(*jitter),
                    status: "响应".into(),
                },
            )
            .collect::<Vec<_>>();
        self.progress = Some(MtrProgress {
            host: self.host.clone(),
            round: 12,
            hops,
        });
        for sample in 0..36 {
            self.hop_history.push(
                rows.iter()
                    .enumerate()
                    .map(|(index, (_, _, _, avg, _, _, _))| {
                        let offset = ((sample + index * 3) % 7) as f64 * 0.06 - 0.18;
                        Some(*avg + offset)
                    })
                    .collect(),
            );
            self.history_times
                .push(format!("14:31:{:02}.000", sample % 60));
        }
    }

    fn render_trends(
        &mut self,
        ui: &mut egui::Ui,
        progress: &MtrProgress,
        actions: &mut Vec<AppAction>,
    ) {
        let active_hops = progress
            .hops
            .iter()
            .enumerate()
            .filter(|(_, hop)| hop.sent > 0)
            .collect::<Vec<_>>();
        if !self.trend_selection_ready {
            self.selected_trend_hops
                .extend(active_hops.iter().map(|(_, hop)| hop.hop));
            self.trend_selection_ready = true;
        }
        let output = export_hops(progress);
        ui::card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("每跳延迟趋势").strong());
                ui.label("选择跳数");
                egui::ComboBox::from_id_salt("mtr-trend-limit")
                    .selected_text(format!("1-{}", self.trend_limit.min(active_hops.len())))
                    .show_ui(ui, |ui| {
                        for limit in [3, 5, 7, 10] {
                            ui.selectable_value(&mut self.trend_limit, limit, format!("1-{limit}"));
                        }
                    });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui::small_action_button(ui, "导出").clicked() {
                        actions.push(AppAction::ExportText {
                            content: output.clone(),
                            file_name: "tooldeck-mtr.log".into(),
                        });
                    }
                    if ui::small_action_button(ui, "复制").clicked() {
                        actions.push(AppAction::CopyText(output.clone()));
                    }
                });
            });
            let colors = mtr_series_colors();
            let selectable_hops = active_hops
                .into_iter()
                .take(self.trend_limit)
                .collect::<Vec<_>>();
            let visible_series = selectable_hops
                .iter()
                .enumerate()
                .filter_map(|(color_index, (history_index, hop))| {
                    self.selected_trend_hops
                        .contains(&hop.hop)
                        .then_some((*history_index, colors[color_index % colors.len()]))
                })
                .collect::<Vec<_>>();
            let legend_width = 150.0;
            let trend_chart_height = 190.0;
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(
                        (ui.available_width() - legend_width).max(320.0),
                        trend_chart_height,
                    ),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        render_mtr_chart(
                            ui,
                            &self.hop_history,
                            &self.history_times,
                            &visible_series,
                            trend_chart_height,
                        );
                    },
                );
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.spacing_mut().interact_size.y = 22.0;
                    ui.add_space(10.0);
                    for (color_index, (_, hop)) in selectable_hops.iter().enumerate() {
                        let color = colors[color_index % colors.len()];
                        let mut selected = self.selected_trend_hops.contains(&hop.hop);
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut selected, "").clicked() {
                                if selected {
                                    self.selected_trend_hops.insert(hop.hop);
                                } else {
                                    self.selected_trend_hops.remove(&hop.hop);
                                }
                            }
                            ui.colored_label(
                                color,
                                format!("{}  {}", hop.hop, hop.address.as_deref().unwrap_or("*")),
                            );
                        });
                    }
                });
            });
        });
    }
}

fn mtr_family_control(
    ui: &mut egui::Ui,
    family: &mut PingAddressFamily,
    palette: ui::ThemePalette,
) {
    ui.vertical(|ui| {
        ui.label(RichText::new("地址族").size(12.0).color(palette.weak));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for value in [
                PingAddressFamily::Auto,
                PingAddressFamily::V4,
                PingAddressFamily::V6,
            ] {
                if ui
                    .add_sized(
                        [58.0, ui::CONTROL_HEIGHT],
                        egui::Button::selectable(*family == value, value.label()),
                    )
                    .clicked()
                {
                    *family = value;
                }
            }
        });
    });
}

fn mtr_round_control(ui: &mut egui::Ui, total_rounds: &mut Option<u32>, palette: ui::ThemePalette) {
    ui.vertical(|ui| {
        ui.label(RichText::new("轮次").size(12.0).color(palette.weak));
        let finite = total_rounds.is_some();
        let mut rounds = total_rounds.unwrap_or(20);
        if ui
            .add_enabled(
                finite,
                egui::DragValue::new(&mut rounds)
                    .range(1..=100)
                    .min_decimals(0),
            )
            .changed()
        {
            *total_rounds = Some(rounds);
        }
    });
}

fn mtr_drag_u32(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u32,
    range: std::ops::RangeInclusive<u32>,
    suffix: &str,
    palette: ui::ThemePalette,
) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).size(12.0).color(palette.weak));
        ui.add(
            egui::DragValue::new(value)
                .range(range)
                .suffix(suffix)
                .min_decimals(0),
        );
    });
}

fn mtr_table_column_widths(available_width: f32) -> [f32; 10] {
    let total = available_width.max(1_140.0);
    let fixed = 54.0 + 104.0 + 108.0 * 5.0 + 96.0;
    let host = (total * 0.15).max(142.0);
    let address = (total - fixed - host).max(176.0);
    [
        54.0, host, address, 104.0, 108.0, 108.0, 108.0, 108.0, 108.0, 96.0,
    ]
}

fn render_mtr_table_header(ui: &mut egui::Ui, column_widths: &[f32; 10]) {
    let palette = ui::palette_for_ui(ui);
    let total_width = column_widths.iter().sum();
    let (response, painter) =
        ui.allocate_painter(egui::vec2(total_width, 32.0), egui::Sense::hover());
    painter.rect_filled(
        response.rect,
        egui::CornerRadius::ZERO,
        ui.visuals().faint_bg_color,
    );
    painter.rect_stroke(
        response.rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.0_f32, palette.border_subtle),
        egui::StrokeKind::Middle,
    );
    for (index, label) in [
        "跳", "主机", "地址", "丢包", "已发", "最新", "平均", "最小", "最大", "抖动",
    ]
    .into_iter()
    .enumerate()
    {
        let cell = mtr_table_cell(response.rect, column_widths, index);
        paint_mtr_table_text(
            &painter,
            cell,
            label,
            egui::FontId::proportional(12.0),
            palette.weak,
        );
        if index > 0 {
            painter.line_segment(
                [
                    egui::pos2(cell.left(), cell.top()),
                    egui::pos2(cell.left(), cell.bottom()),
                ],
                egui::Stroke::new(1.0_f32, palette.border_subtle),
            );
        }
    }
}

fn render_mtr_table_row(
    ui: &mut egui::Ui,
    hop: &MtrHopStats,
    column_widths: &[f32; 10],
    selected_hop: &mut Option<u8>,
) {
    let palette = ui::palette_for_ui(ui);
    let total_width = column_widths.iter().sum();
    let (response, painter) =
        ui.allocate_painter(egui::vec2(total_width, 38.0), egui::Sense::click());
    if response.clicked() {
        *selected_hop = Some(hop.hop);
    }
    let selected = *selected_hop == Some(hop.hop);
    let fill = if selected {
        palette.accent.gamma_multiply(0.26)
    } else if usize::from(hop.hop) % 2 == 0 {
        ui.visuals().faint_bg_color.gamma_multiply(0.55)
    } else {
        egui::Color32::TRANSPARENT
    };
    painter.rect_filled(response.rect, egui::CornerRadius::ZERO, fill);
    painter.rect_stroke(
        response.rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.0_f32, palette.border_subtle),
        egui::StrokeKind::Middle,
    );
    let values = [
        hop.hop.to_string(),
        hop.hostname.as_deref().unwrap_or("-").to_owned(),
        hop.address.as_deref().unwrap_or("*").to_owned(),
        format!("{:.1}%", hop.loss_percent()),
        hop.sent.to_string(),
        format_mtr_table_ms(hop.last_ms),
        format_mtr_table_ms(hop.avg_ms),
        format_mtr_table_ms(hop.min_ms),
        format_mtr_table_ms(hop.max_ms),
        format_mtr_table_ms(hop.jitter_ms),
    ];
    for (index, value) in values.iter().enumerate() {
        let cell = mtr_table_cell(response.rect, column_widths, index);
        paint_mtr_table_text(
            &painter,
            cell,
            value,
            egui::FontId::monospace(12.0),
            palette.text,
        );
        if index > 0 {
            painter.line_segment(
                [
                    egui::pos2(cell.left(), cell.top()),
                    egui::pos2(cell.left(), cell.bottom()),
                ],
                egui::Stroke::new(1.0_f32, palette.border_subtle),
            );
        }
    }
}

fn mtr_table_cell(row: egui::Rect, widths: &[f32; 10], index: usize) -> egui::Rect {
    let left = row.left() + widths[..index].iter().sum::<f32>();
    egui::Rect::from_min_size(
        egui::pos2(left, row.top()),
        egui::vec2(widths[index], row.height()),
    )
}

fn paint_mtr_table_text(
    painter: &egui::Painter,
    cell: egui::Rect,
    value: &str,
    font: egui::FontId,
    color: egui::Color32,
) {
    painter
        .with_clip_rect(cell.shrink2(egui::vec2(7.0, 2.0)))
        .text(
            cell.left_center() + egui::vec2(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            value,
            font,
            color,
        );
}

fn mtr_series_colors() -> [egui::Color32; 7] {
    [
        egui::Color32::from_rgb(72, 210, 105),
        egui::Color32::from_rgb(236, 215, 55),
        egui::Color32::from_rgb(36, 191, 216),
        egui::Color32::from_rgb(49, 139, 255),
        egui::Color32::from_rgb(180, 91, 232),
        egui::Color32::from_rgb(255, 145, 32),
        egui::Color32::from_rgb(238, 70, 76),
    ]
}

fn render_mtr_chart(
    ui: &mut egui::Ui,
    history: &[Vec<Option<f64>>],
    history_times: &[String],
    visible_series: &[(usize, egui::Color32)],
    height: f32,
) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(320.0), height),
        egui::Sense::hover(),
    );
    let palette = ui::palette_for_ui(ui);
    let plot = egui::Rect::from_min_max(
        rect.min + egui::vec2(36.0, 20.0),
        rect.max - egui::vec2(8.0, 24.0),
    );
    let grid = palette.border.gamma_multiply(0.7);
    ui.painter().text(
        rect.left_top(),
        egui::Align2::LEFT_TOP,
        "延迟 (ms)",
        egui::FontId::proportional(11.0),
        palette.weak,
    );
    let maximum = (history
        .iter()
        .flat_map(|sample| {
            visible_series
                .iter()
                .filter_map(|(index, _)| sample.get(*index).copied().flatten())
        })
        .fold(1.0_f64, f64::max)
        * 1.15
        / 5.0)
        .ceil()
        .max(1.0)
        * 5.0;
    for index in 0..=4 {
        let y = egui::lerp(plot.bottom()..=plot.top(), index as f32 / 4.0);
        ui.painter().line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            egui::Stroke::new(1.0_f32, grid),
        );
        ui.painter().text(
            egui::pos2(plot.left() - 6.0, y),
            egui::Align2::RIGHT_CENTER,
            format!("{:.0}", maximum * f64::from(index) / 4.0),
            egui::FontId::monospace(10.0),
            palette.weak,
        );
    }
    let divisor = history.len().saturating_sub(1).max(1) as f32;
    for &(series, color) in visible_series {
        let points = history
            .iter()
            .enumerate()
            .filter_map(|(index, sample)| {
                sample.get(series).copied().flatten().map(|value| {
                    egui::pos2(
                        egui::lerp(plot.left()..=plot.right(), index as f32 / divisor),
                        egui::lerp(plot.bottom()..=plot.top(), (value / maximum) as f32),
                    )
                })
            })
            .collect::<Vec<_>>();
        if points.len() > 1 {
            ui.painter()
                .add(egui::Shape::line(points, egui::Stroke::new(1.6_f32, color)));
        }
    }
    if !history_times.is_empty() {
        for index in [0, history_times.len() / 2, history_times.len() - 1] {
            let x = egui::lerp(plot.left()..=plot.right(), index as f32 / divisor);
            ui.painter().text(
                egui::pos2(x, plot.bottom() + 8.0),
                egui::Align2::CENTER_TOP,
                chart_time_label(&history_times[index]),
                egui::FontId::monospace(10.0),
                palette.weak,
            );
        }
    }
}

fn chart_time_label(time: &str) -> &str {
    time.split_once('.').map_or(time, |(label, _)| label)
}

fn export_hops(progress: &MtrProgress) -> String {
    let mut output =
        String::from("跳数\t地址\t发送\t接收\t丢包\t最近\t平均\t最小\t最大\t抖动\t状态\n");
    for hop in progress.hops.iter().filter(|hop| hop.sent > 0) {
        output.push_str(&format!(
            "{}\t{}\t{}\t{}\t{:.1}%\t{}\t{}\t{}\t{}\t{}\t{}\n",
            hop.hop,
            hop.address.as_deref().unwrap_or("*"),
            hop.sent,
            hop.received,
            hop.loss_percent(),
            format_ms(hop.last_ms),
            format_ms(hop.avg_ms),
            format_ms(hop.min_ms),
            format_ms(hop.max_ms),
            format_ms(hop.jitter_ms),
            hop.status
        ));
    }
    output
}

fn format_ms(value: Option<f64>) -> String {
    value.map_or_else(|| "-".into(), |value| format!("{value:.1} ms"))
}

fn format_mtr_table_ms(value: Option<f64>) -> String {
    value.map_or_else(|| "-".into(), |value| format!("{value:.3}"))
}

#[cfg(test)]
mod tests {
    use super::{MtrTool, export_hops};
    use crate::model::{MtrHopStats, MtrProgress};

    #[test]
    fn trend_history_keeps_latest_one_hundred_twenty_snapshots() {
        let mut tool = MtrTool::default();
        for round in 1..=125 {
            tool.push_history(&MtrProgress {
                host: "example.test".into(),
                round,
                hops: vec![MtrHopStats::new(1)],
            });
        }

        assert_eq!(tool.hop_history.len(), 120);
    }

    #[test]
    fn export_only_contains_probed_hops_and_keeps_repeated_addresses() {
        let mut first = MtrHopStats::new(1);
        first.record(Some("192.0.2.1".into()), Some(1.0), "中间跳");
        let untouched = MtrHopStats::new(2);
        let mut repeated = MtrHopStats::new(3);
        repeated.record(Some("192.0.2.1".into()), Some(3.0), "目标响应");
        let output = export_hops(&MtrProgress {
            host: "example.test".into(),
            round: 1,
            hops: vec![first, untouched, repeated],
        });

        assert!(output.starts_with("跳数\t地址"));
        assert!(output.contains("1\t192.0.2.1"));
        assert!(!output.contains("2\t*"));
        assert!(output.contains("3\t192.0.2.1"));
    }
}
