//! DNS、Ping、TCP 握手测试工具页面。

use crate::{
    core::{actions::AppAction, invocation::ToolPayload, worker::TaskResult},
    model::{
        DnsRecord, DnsRecordType, DnsResult, PingAddressFamily, PingConfig, PingProgress,
        PingSample, TcpProbeConfig, TcpProbeProgress,
    },
    platform::windows::{local_date_time_millis, local_time_hms_millis},
    tools::{
        registry::{ToolCategory, ToolDescriptor, ToolIcon},
        ToolModule, ToolUiContext,
    },
    ui,
};
use eframe::egui::{self, RichText};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub struct DnsLookupTool {
    host: String,
    record_type: DnsRecordType,
    bypass_cache: bool,
    periodic: bool,
    periodic_active: bool,
    interval_seconds: u32,
    next_run: Option<Instant>,
    filter: String,
    history: VecDeque<DnsHistoryEntry>,
    last_result_time: Option<String>,
    pending_record_type: DnsRecordType,
    busy: bool,
    result: Option<Result<DnsResult, crate::model::AppError>>,
    review_seeded: bool,
}

#[derive(Clone)]
struct DnsHistoryEntry {
    timestamp: String,
    host: String,
    record_type: DnsRecordType,
    record_count: usize,
    elapsed_ms: u64,
}

impl Default for DnsLookupTool {
    fn default() -> Self {
        Self {
            host: String::new(),
            record_type: DnsRecordType::Auto,
            bypass_cache: false,
            periodic: false,
            periodic_active: false,
            interval_seconds: 5,
            next_run: None,
            filter: String::new(),
            history: VecDeque::new(),
            last_result_time: None,
            pending_record_type: DnsRecordType::Auto,
            busy: false,
            result: None,
            review_seeded: false,
        }
    }
}
pub struct PingTool {
    host: String,
    count: u32,
    timeout_ms: u32,
    payload_size: u16,
    interval_ms: u32,
    continuous: bool,
    family: PingAddressFamily,
    busy: bool,
    progress: Option<PingProgress>,
    live_samples: Vec<PingSample>,
    sample_times: Vec<String>,
    result: Option<Result<crate::model::PingSummary, crate::model::AppError>>,
    review_seeded: bool,
}
pub struct TcpProbeTool {
    host: String,
    port: u16,
    timeout_ms: u32,
    attempts: u32,
    interval_ms: u32,
    family: PingAddressFamily,
    busy: bool,
    progress: Vec<TcpProbeProgress>,
    attempt_times: Vec<String>,
    result: Option<Result<crate::model::TcpProbeResult, crate::model::AppError>>,
    review_seeded: bool,
}

impl Default for PingTool {
    fn default() -> Self {
        Self {
            host: String::new(),
            count: 4,
            timeout_ms: 1_000,
            payload_size: 32,
            interval_ms: 1_000,
            continuous: false,
            family: PingAddressFamily::Auto,
            busy: false,
            progress: None,
            live_samples: Vec::new(),
            sample_times: Vec::new(),
            result: None,
            review_seeded: false,
        }
    }
}

impl Default for TcpProbeTool {
    fn default() -> Self {
        Self {
            host: String::new(),
            port: 443,
            timeout_ms: 3_000,
            attempts: 3,
            interval_ms: 1_000,
            family: PingAddressFamily::Auto,
            busy: false,
            progress: Vec::new(),
            attempt_times: Vec::new(),
            result: None,
            review_seeded: false,
        }
    }
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
    fn ui(&mut self, ui: &mut egui::Ui, context: ToolUiContext) -> Vec<AppAction> {
        if context.review_mode && !self.review_seeded {
            self.seed_review();
        }
        let mut actions = Vec::new();
        let palette = ui::palette_for_ui(ui);
        ui::page_heading(ui, "DNS 查询", "使用系统 DNS 解析域名记录");
        ui.add_space(ui::SPACE_12);
        ui::card(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let mut submit = false;
            ui.horizontal_wrapped(|ui| {
                let input = ui.add_sized(
                    [290.0_f32.min(ui.available_width()), ui::CONTROL_HEIGHT],
                    ui::text_input(&mut self.host, "输入域名或 IP 地址"),
                );
                submit |=
                    input.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                egui::ComboBox::from_id_salt("dns-type")
                    .width(260.0)
                    .selected_text(if self.record_type == DnsRecordType::Auto {
                        "A / AAAA / CNAME / MX / TXT"
                    } else {
                        self.record_type.label()
                    })
                    .show_ui(ui, |ui| {
                        for value in DnsRecordType::ALL {
                            ui.selectable_value(&mut self.record_type, value, value.label());
                        }
                    });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if ui
                        .add_sized(
                            [72.0, ui::CONTROL_HEIGHT],
                            egui::Button::selectable(!self.periodic, "单次"),
                        )
                        .clicked()
                    {
                        self.periodic = false;
                    }
                    if ui
                        .add_sized(
                            [72.0, ui::CONTROL_HEIGHT],
                            egui::Button::selectable(self.periodic, "周期"),
                        )
                        .clicked()
                    {
                        self.periodic = true;
                    }
                });
                ui.label("间隔");
                ui.add_enabled(
                    self.periodic,
                    egui::DragValue::new(&mut self.interval_seconds)
                        .range(1..=3600)
                        .suffix(" 秒"),
                );
                ui.add_enabled_ui(!self.busy, |ui| {
                    if ui::primary_button_sized(ui, "查询", [78.0, ui::CONTROL_HEIGHT]).clicked() {
                        submit = true;
                    }
                });
                let is_active = self.busy || self.periodic_active;
                if ui::danger_outline_button_sized(ui, "停止", [78.0, ui::CONTROL_HEIGHT], is_active)
                    .clicked()
                    && is_active
                {
                    self.periodic_active = false;
                    self.next_run = None;
                    actions.push(AppAction::StopDns);
                }
            });
            ui.add_space(17.0);
            ui.horizontal(|ui| {
                let (dot_rect, _) =
                    ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter()
                    .circle_filled(dot_rect.center(), 4.0, palette.success_text);
                ui.label("系统 DNS");
                let elapsed = self
                    .result
                    .as_ref()
                    .and_then(|result| result.as_ref().ok())
                    .map_or_else(
                        || "尚未查询".to_owned(),
                        |result| format!("上次耗时 {} ms", result.elapsed_ms),
                    );
                ui.label(RichText::new(elapsed).color(palette.weak));
                if self.periodic_active {
                    let (running_rect, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter()
                        .circle_filled(running_rect.center(), 4.0, palette.accent);
                    ui.label(RichText::new("周期查询中").color(palette.accent));
                }
            });
            if submit && !self.busy {
                self.periodic_active = self.periodic;
                actions.push(self.run_action());
            }
        });
        if let Some(Err(error)) = &self.result {
            ui.add_space(ui::SPACE_12);
            state(ui, self.busy, Some(error));
        }
        if self.result.is_some() || !self.history.is_empty() {
            ui.add_space(0.0);
            self.render_results(ui, &mut actions);
        } else if self.busy {
            ui.add_space(ui::SPACE_12);
            state(ui, true, None);
        }
        actions
    }
    fn handle_invocation(&mut self, payload: ToolPayload) -> Vec<AppAction> {
        if let ToolPayload::Host { host } = payload {
            self.host = host;
            return vec![self.run_action()];
        }
        Vec::new()
    }
    fn handle_task_result(&mut self, result: TaskResult) {
        if let TaskResult::Dns(result) = result {
            self.busy = false;
            if let Ok(value) = &result {
                let timestamp = local_time_hms_millis();
                self.last_result_time = Some(timestamp.clone());
                self.history.push_front(DnsHistoryEntry {
                    timestamp,
                    host: value.host.clone(),
                    record_type: self.pending_record_type,
                    record_count: value.records.len(),
                    elapsed_ms: value.elapsed_ms,
                });
                self.history.truncate(20);
            }
            self.result = Some(result);
            self.next_run = self
                .periodic_active
                .then(|| Instant::now() + Duration::from_secs(u64::from(self.interval_seconds)));
        }
    }
    fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }
    fn poll_actions(&mut self, now: Instant) -> Vec<AppAction> {
        if self.periodic_active && !self.busy && self.next_run.is_some_and(|next| now >= next) {
            self.next_run = Some(now + Duration::from_secs(u64::from(self.interval_seconds)));
            return vec![self.run_action()];
        }
        Vec::new()
    }
}

impl DnsLookupTool {
    fn run_action(&mut self) -> AppAction {
        self.pending_record_type = self.record_type;
        AppAction::RunDns {
            host: self.host.clone(),
            record_type: self.record_type,
            bypass_cache: self.bypass_cache,
        }
    }

    fn seed_review(&mut self) {
        self.review_seeded = true;
        self.host = "www.example.com".into();
        self.record_type = DnsRecordType::Auto;
        self.pending_record_type = DnsRecordType::Auto;
        self.periodic = true;
        self.periodic_active = true;
        self.busy = true;
        self.last_result_time = Some("14:32:18.352".into());
        self.result = Some(Ok(DnsResult {
            host: self.host.clone(),
            elapsed_ms: 18,
            records: vec![
                DnsRecord {
                    name: self.host.clone(),
                    record_type: DnsRecordType::A,
                    value: "192.0.2.80".into(),
                    ttl: 300,
                },
                DnsRecord {
                    name: self.host.clone(),
                    record_type: DnsRecordType::Aaaa,
                    value: "2001:db8::80".into(),
                    ttl: 300,
                },
            ],
        }));
        for (timestamp, record_type, count, elapsed_ms) in [
            ("14:32:18", DnsRecordType::Auto, 2, 18),
            ("14:31:45", DnsRecordType::A, 1, 17),
            ("14:31:20", DnsRecordType::Aaaa, 1, 16),
            ("14:30:55", DnsRecordType::Mx, 0, 19),
            ("14:30:30", DnsRecordType::Txt, 1, 20),
            ("14:29:58", DnsRecordType::Cname, 1, 16),
            ("14:29:24", DnsRecordType::A, 1, 17),
            ("14:28:51", DnsRecordType::Aaaa, 1, 16),
            ("14:28:18", DnsRecordType::Mx, 0, 18),
            ("14:27:45", DnsRecordType::Txt, 1, 19),
        ] {
            self.history.push_back(DnsHistoryEntry {
                timestamp: timestamp.into(),
                host: self.host.clone(),
                record_type,
                record_count: count,
                elapsed_ms,
            });
        }
    }

    fn render_results(&mut self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        let available = ui.available_width();
        let panel_height = (ui.ctx().screen_rect().height() * 0.58).clamp(420.0, 620.0);
        if available < 820.0 {
            self.render_record_panel(ui, actions, panel_height * 0.62);
            ui.add_space(ui::SPACE_12);
            self.render_history_panel(ui, panel_height * 0.48);
            return;
        }
        let gap = ui::SPACE_12;
        let available_width = ui.available_width();
        let content_width = (available_width - gap).max(0.0);
        let record_width = content_width * 0.58;
        let history_width = content_width - record_width;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.allocate_ui_with_layout(
                egui::vec2(record_width, panel_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.render_record_panel(ui, actions, panel_height),
            );
            ui.allocate_ui_with_layout(
                egui::vec2(history_width, panel_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.render_history_panel(ui, panel_height),
            );
        });
    }

    fn render_record_panel(
        &mut self,
        ui: &mut egui::Ui,
        actions: &mut Vec<AppAction>,
        height: f32,
    ) {
        let palette = ui::palette_for_ui(ui);
        let result = self.result.as_ref().and_then(|result| result.as_ref().ok());
        ui::card(ui, |ui| {
            ui.set_min_height(height - ui::SPACE_16 * 2.0);
            ui.horizontal(|ui| {
                ui.add_sized(
                    [220.0_f32.min(ui.available_width()), ui::CONTROL_HEIGHT],
                    ui::text_input(&mut self.filter, "筛选结果"),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(result) = result {
                        let content = dns_record_lines(result).join("\n");
                        if ui::small_action_button(ui, "⤤ 导出").clicked() {
                            actions.push(AppAction::ExportText {
                                content: content.clone(),
                                file_name: "tooldeck-dns.log".into(),
                            });
                        }
                        if ui::small_action_button(ui, "📋 复制").clicked() {
                            actions.push(AppAction::CopyText(content));
                        }
                    }
                });
            });
            ui.add_space(ui::SPACE_8);
            ui.separator();
            let filter = self.filter.trim().to_ascii_lowercase();
            let timestamp = self.last_result_time.as_deref().unwrap_or("-");
            let column_widths = dns_result_column_widths(ui.available_width());
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::ScrollArea::both()
                    .max_height((height - 116.0).max(180.0))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        render_fixed_table_header(
                            ui,
                            &column_widths,
                            &["时间", "类型", "名称", "值", "TTL", "耗时"],
                        );
                        ui.spacing_mut().item_spacing.y = 0.0;
                        if let Some(result) = result {
                            for (index, record) in result
                                .records
                                .iter()
                                .filter(|record| {
                                    filter.is_empty()
                                        || record.name.to_ascii_lowercase().contains(&filter)
                                        || record.value.to_ascii_lowercase().contains(&filter)
                                        || record
                                            .record_type
                                            .label()
                                            .to_ascii_lowercase()
                                            .contains(&filter)
                                })
                                .enumerate()
                            {
                                render_fixed_table_row(
                                    ui,
                                    &column_widths,
                                    index,
                                    &[
                                        timestamp.into(),
                                        record.record_type.label().into(),
                                        record.name.clone(),
                                        record.value.clone(),
                                        record.ttl.to_string(),
                                        format!("{} ms", result.elapsed_ms),
                                    ],
                                    &[
                                        palette.text,
                                        palette.text,
                                        palette.text,
                                        palette.accent,
                                        palette.text,
                                        palette.text,
                                    ],
                                );
                            }
                        }
                    });
            });
        });
    }

    fn render_history_panel(&self, ui: &mut egui::Ui, height: f32) {
        let palette = ui::palette_for_ui(ui);
        ui::card(ui, |ui| {
            ui.set_min_height(height - ui::SPACE_16 * 2.0);
            ui.label(RichText::new("当前会话历史").strong().size(15.0));
            ui.add_space(ui::SPACE_8);
            ui.separator();
            let column_widths = dns_history_column_widths(ui.available_width());
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::ScrollArea::both()
                    .max_height((height - 94.0).max(160.0))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        render_fixed_table_header(
                            ui,
                            &column_widths,
                            &["时间", "域名", "类型", "结果数", "耗时"],
                        );
                        ui.spacing_mut().item_spacing.y = 0.0;
                        for (index, entry) in self.history.iter().enumerate() {
                            render_fixed_table_row(
                                ui,
                                &column_widths,
                                index,
                                &[
                                    entry.timestamp.clone(),
                                    entry.host.clone(),
                                    entry.record_type.label().into(),
                                    entry.record_count.to_string(),
                                    format!("{} ms", entry.elapsed_ms),
                                ],
                                &[
                                    palette.text,
                                    palette.text,
                                    palette.text,
                                    palette.text,
                                    palette.text,
                                ],
                            );
                        }
                    });
            });
        });
    }
}

fn dns_result_column_widths(available_width: f32) -> [f32; 6] {
    let total = available_width.max(580.0);
    let time = 118.0;
    let record_type = 66.0;
    let name = (total * 0.22).max(130.0);
    let ttl = 64.0;
    let elapsed = 74.0;
    let value = (total - time - record_type - name - ttl - elapsed).max(116.0);
    [time, record_type, name, value, ttl, elapsed]
}

fn dns_history_column_widths(available_width: f32) -> [f32; 5] {
    let total = available_width.max(420.0);
    let time = 112.0;
    let record_type = 66.0;
    let count = 68.0;
    let elapsed = 74.0;
    let host = (total - time - record_type - count - elapsed).max(120.0);
    [time, host, record_type, count, elapsed]
}

fn dns_record_lines(result: &DnsResult) -> Vec<String> {
    result
        .records
        .iter()
        .map(|record| {
            format!(
                "{}\t{}\t{}\tTTL {}",
                record.name,
                record.record_type.label(),
                record.value,
                record.ttl
            )
        })
        .collect()
}

#[allow(dead_code)]
fn labeled_family_control(
    ui: &mut egui::Ui,
    id: &'static str,
    label: &str,
    family: &mut PingAddressFamily,
    palette: ui::ThemePalette,
) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).size(12.0).color(palette.weak));
        ui.horizontal(|ui| {
            ui.push_id(id, |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for value in [
                    PingAddressFamily::Auto,
                    PingAddressFamily::V4,
                    PingAddressFamily::V6,
                ] {
                    ui.add_sized(
                        [58.0, ui::CONTROL_HEIGHT],
                        egui::Button::selectable(*family == value, value.label()),
                    )
                    .clicked()
                    .then(|| *family = value);
                }
            });
        });
    });
}

#[allow(dead_code)]
fn labeled_drag_u32(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u32,
    range: std::ops::RangeInclusive<u32>,
    suffix: &str,
    enabled: bool,
) {
    let weak = ui::palette_for_ui(ui).weak;
    ui.vertical(|ui| {
        ui.label(RichText::new(label).size(12.0).color(weak));
        ui.add_enabled(
            enabled,
            egui::DragValue::new(value)
                .range(range)
                .suffix(suffix)
                .min_decimals(0),
        );
    });
}

fn format_latency(value: Option<f64>) -> String {
    value.map_or_else(|| "-".into(), |value| format!("{value:.1} ms"))
}

fn render_statistic_strip(ui: &mut egui::Ui, values: &[(&str, String, egui::Color32)]) {
    if values.is_empty() {
        return;
    }

    let palette = ui::palette_for_ui(ui);
    let (response, painter) = ui.allocate_painter(
        egui::vec2(ui.available_width(), 48.0),
        egui::Sense::hover(),
    );
    let cell_width = response.rect.width() / values.len() as f32;
    for (index, (label, value, color)) in values.iter().enumerate() {
        let cell = egui::Rect::from_min_size(
            egui::pos2(response.rect.left() + cell_width * index as f32, response.rect.top()),
            egui::vec2(cell_width, response.rect.height()),
        );
        painter.text(
            cell.left_top() + egui::vec2(12.0, 8.0),
            egui::Align2::LEFT_TOP,
            label,
            egui::FontId::proportional(12.0),
            palette.weak,
        );
        painter.text(
            cell.left_bottom() + egui::vec2(12.0, -8.0),
            egui::Align2::LEFT_BOTTOM,
            value,
            egui::FontId::monospace(20.0),
            *color,
        );
        if index > 0 {
            painter.line_segment(
                [
                    egui::pos2(cell.left(), cell.top() + 4.0),
                    egui::pos2(cell.left(), cell.bottom() - 4.0),
                ],
                egui::Stroke::new(1.0_f32, palette.border_subtle),
            );
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "图表绘制参数按调用点完整展开以保持数值含义直观"
)]
fn render_line_chart(
    ui: &mut egui::Ui,
    values: &[Option<f64>],
    x_labels: &[String],
    height: f32,
    color: egui::Color32,
    maximum_override: Option<f64>,
    grid_steps: usize,
    highlighted_indices: &[usize],
) {
    let width = ui.available_width().max(240.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let palette = ui::palette_for_ui(ui);
    let plot = egui::Rect::from_min_max(
        rect.min + egui::vec2(42.0, 18.0),
        rect.max - egui::vec2(14.0, 30.0),
    );
    let grid = palette.border.gamma_multiply(0.7);
    let maximum = maximum_override.unwrap_or_else(|| chart_axis_maximum(values));
    let steps = grid_steps.max(1);

    ui.painter().line_segment(
        [
            egui::pos2(plot.left(), plot.top()),
            egui::pos2(plot.left(), plot.bottom()),
        ],
        egui::Stroke::new(1.0_f32, palette.border),
    );
    ui.painter().line_segment(
        [
            egui::pos2(plot.left(), plot.bottom()),
            egui::pos2(plot.right(), plot.bottom()),
        ],
        egui::Stroke::new(1.0_f32, palette.border),
    );
    for index in 0..=steps {
        let y = egui::lerp(plot.bottom()..=plot.top(), index as f32 / steps as f32);
        ui.painter().line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            egui::Stroke::new(1.0_f32, grid),
        );
        ui.painter().text(
            egui::pos2(plot.left() - 8.0, y),
            egui::Align2::RIGHT_CENTER,
            chart_axis_label(maximum * index as f64 / steps as f64),
            egui::FontId::monospace(10.0),
            palette.weak,
        );
    }
    let divisor = values.len().saturating_sub(1).max(1) as f32;
    let mut previous = None;
    for (index, value) in values.iter().enumerate() {
        let Some(value) = value else {
            previous = None;
            continue;
        };
        let x = egui::lerp(plot.left()..=plot.right(), index as f32 / divisor);
        let y = egui::lerp(
            plot.bottom()..=plot.top(),
            (value.min(maximum) / maximum) as f32,
        );
        let point = egui::pos2(x, y);
        if let Some(previous) = previous {
            let segment_color = if highlighted_indices.contains(&index)
                || highlighted_indices.contains(&index.saturating_sub(1))
            {
                palette.danger_text
            } else {
                color
            };
            ui.painter().line_segment(
                [previous, point],
                egui::Stroke::new(1.8_f32, segment_color),
            );
        }
        let point_color = if highlighted_indices.contains(&index) {
            palette.danger_text
        } else {
            color
        };
        ui.painter().circle_filled(point, 3.0, point_color);
        previous = Some(point);
    }

    let tick_indices = chart_tick_indices(values.len());
    for index in tick_indices {
        let x = egui::lerp(plot.left()..=plot.right(), index as f32 / divisor);
        ui.painter().line_segment(
            [
                egui::pos2(x, plot.bottom()),
                egui::pos2(x, plot.bottom() + 4.0),
            ],
            egui::Stroke::new(1.0_f32, palette.border),
        );
        let label = x_labels.get(index).map_or("-", String::as_str);
        ui.painter().text(
            egui::pos2(x, plot.bottom() + 10.0),
            egui::Align2::CENTER_TOP,
            chart_time_label(label),
            egui::FontId::monospace(10.0),
            palette.weak,
        );
    }
}

fn chart_axis_maximum(values: &[Option<f64>]) -> f64 {
    let raw = values.iter().flatten().copied().fold(1.0_f64, f64::max) * 1.10;
    let magnitude = 10_f64.powf(raw.log10().floor());
    for factor in [1.0_f64, 2.0, 5.0, 10.0] {
        let candidate = factor * magnitude;
        if candidate >= raw {
            return candidate;
        }
    }
    raw
}

fn chart_axis_label(value: f64) -> String {
    if value.abs() < f64::EPSILON {
        "0".into()
    } else if value < 10.0 {
        format!("{value:.1}")
    } else {
        format!("{value:.0}")
    }
}

fn chart_tick_indices(value_count: usize) -> Vec<usize> {
    if value_count <= 1 {
        return vec![0];
    }
    if value_count <= 10 {
        return (0..value_count).collect();
    }
    let last = value_count - 1;
    (0..=5).map(|step| last * step / 5).collect()
}

fn chart_time_label(value: &str) -> &str {
    value.split_once('.').map_or(value, |(time, _)| time)
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
    fn ui(&mut self, ui: &mut egui::Ui, context: ToolUiContext) -> Vec<AppAction> {
        if context.review_mode && !self.review_seeded {
            self.seed_review();
        }
        let mut actions = Vec::new();
        let palette = ui::palette_for_ui(ui);
        ui::page_heading(ui, "Ping", "持续观察 ICMP 回显与延迟变化");
        ui.add_space(ui::SPACE_12);
        ui::card(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let mut submit = false;
            let field_widths = [190.0, 174.0, 128.0, 64.0, 74.0, 74.0, 78.0, 78.0];
            ui::responsive_parameter_row(ui, &field_widths, ui::SPACE_8, |ui| {
                ui::parameter_group(ui, 190.0, |ui| {
                    ui.add_sized(
                        [190.0, 16.0],
                        egui::Label::new(
                            RichText::new("目标地址").size(12.0).color(palette.weak),
                        ),
                    );
                    let input = ui.add_sized(
                        [190.0, ui::CONTROL_HEIGHT],
                        ui::text_input(&mut self.host, "主机或 IP"),
                    );
                    submit |=
                        input.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                });

                ui::parameter_group(ui, 174.0, |ui| {
                    ui.add_sized(
                        [174.0, 16.0],
                        egui::Label::new(
                            RichText::new("地址族").size(12.0).color(palette.weak),
                        ),
                    );
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
                                    egui::Button::selectable(self.family == value, value.label()),
                                )
                                .clicked()
                            {
                                self.family = value;
                            }
                        }
                    });
                });

                ui::parameter_group(ui, 128.0, |ui| {
                    ui.add_sized(
                        [128.0, 16.0],
                        egui::Label::new(RichText::new("模式").size(12.0).color(palette.weak)),
                    );
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        if ui
                            .add_sized(
                                [64.0, ui::CONTROL_HEIGHT],
                                egui::Button::selectable(!self.continuous, "有限"),
                            )
                            .clicked()
                        {
                            self.continuous = false;
                        }
                        if ui
                            .add_sized(
                                [64.0, ui::CONTROL_HEIGHT],
                                egui::Button::selectable(self.continuous, "持续"),
                            )
                            .clicked()
                        {
                            self.continuous = true;
                        }
                    });
                });

                ui::parameter_group(ui, 64.0, |ui| {
                    ui.add_sized(
                        [64.0, 16.0],
                        egui::Label::new(RichText::new("次数").size(12.0).color(palette.weak)),
                    );
                    ui.add_enabled_ui(!self.continuous, |ui| {
                        ui.add_sized(
                            [64.0, ui::CONTROL_HEIGHT],
                            egui::DragValue::new(&mut self.count).range(1..=100),
                        );
                    });
                });

                ui::parameter_group(ui, 74.0, |ui| {
                    ui.add_sized(
                        [74.0, 16.0],
                        egui::Label::new(
                            RichText::new("超时(ms)").size(12.0).color(palette.weak),
                        ),
                    );
                    ui.add_sized(
                        [74.0, ui::CONTROL_HEIGHT],
                        egui::DragValue::new(&mut self.timeout_ms).range(100..=10_000),
                    );
                });

                ui::parameter_group(ui, 74.0, |ui| {
                    ui.add_sized(
                        [74.0, 16.0],
                        egui::Label::new(
                            RichText::new("间隔(ms)").size(12.0).color(palette.weak),
                        ),
                    );
                    ui.add_sized(
                        [74.0, ui::CONTROL_HEIGHT],
                        egui::DragValue::new(&mut self.interval_ms).range(100..=60_000),
                    );
                });

                ui::parameter_group(ui, 78.0, |ui| {
                    ui.allocate_space(egui::vec2(78.0, 16.0));
                    ui.add_enabled_ui(!self.busy, |ui| {
                        submit |= ui::primary_button_sized(
                            ui,
                            "开始",
                            [78.0, ui::CONTROL_HEIGHT],
                        )
                        .clicked();
                    });
                });

                ui::parameter_group(ui, 78.0, |ui| {
                    ui.allocate_space(egui::vec2(78.0, 16.0));
                    if ui::danger_outline_button_sized(
                        ui,
                        "停止",
                        [78.0, ui::CONTROL_HEIGHT],
                        self.busy,
                    )
                    .clicked()
                        && self.busy
                    {
                        actions.push(AppAction::StopPing);
                    }
                });
            });
            if submit && !self.busy {
                actions.push(AppAction::RunPing {
                    host: self.host.clone(),
                    config: self.config(),
                });
            }
        });
        if let Some(Err(error)) = &self.result {
            ui.add_space(ui::SPACE_12);
            state(ui, self.busy, Some(error));
        }
        let samples = self.display_samples();
        if !samples.is_empty() {
            ui.add_space(0.0);
            self.render_ping_workspace(ui, samples, palette);
        } else if self.busy {
            ui.add_space(ui::SPACE_12);
            state(ui, true, None);
        }
        actions
    }
    fn handle_invocation(&mut self, payload: ToolPayload) -> Vec<AppAction> {
        if let ToolPayload::Host { host } = payload {
            self.host = host;
            return vec![AppAction::RunPing {
                host: self.host.clone(),
                config: self.config(),
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
    fn handle_ping_progress(&mut self, progress: PingProgress) {
        self.live_samples.push(progress.sample.clone());
        self.sample_times.push(local_time_hms_millis());
        if self.live_samples.len() > 500 {
            self.live_samples.remove(0);
            self.sample_times.remove(0);
        }
        self.progress = Some(progress);
    }
    fn set_busy(&mut self, busy: bool) {
        if busy {
            self.progress = None;
            self.live_samples.clear();
            self.sample_times.clear();
        }
        self.busy = busy;
    }
    fn poll_actions(&mut self, _now: Instant) -> Vec<AppAction> {
        Vec::new()
    }
}

impl PingTool {
    fn config(&self) -> PingConfig {
        PingConfig {
            family: self.family,
            count: self.count.clamp(1, 100),
            timeout_ms: self.timeout_ms.clamp(100, 10_000),
            payload_size: self.payload_size.min(1_472),
            interval_ms: self.interval_ms.clamp(100, 60_000),
            continuous: self.continuous,
        }
    }

    fn display_samples(&self) -> &[PingSample] {
        if !self.live_samples.is_empty() {
            &self.live_samples
        } else {
            self.result
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .map_or(&[], |result| result.samples.as_slice())
        }
    }

    fn statistics(&self) -> (u32, u32, Option<f64>, Option<f64>, Option<f64>) {
        if let Some(progress) = &self.progress {
            return (
                progress.sent,
                progress.received,
                progress.min_ms,
                progress.avg_ms,
                progress.max_ms,
            );
        }
        self.result
            .as_ref()
            .and_then(|result| result.as_ref().ok())
            .map_or((0, 0, None, None, None), |result| {
                (
                    result.sent,
                    result.received,
                    result.min_ms,
                    result.avg_ms,
                    result.max_ms,
                )
            })
    }

    fn seed_review(&mut self) {
        self.review_seeded = true;
        self.host = "127.0.0.1".into();
        self.payload_size = 64;
        self.continuous = true;
        self.count = 10;
        self.busy = true;
        let latencies = [
            0.4, 0.6, 0.5, 0.4, 0.7, 0.7, 0.5, 0.5, 0.8, 0.6, 0.7, 0.5, 0.4, 0.6, 0.6, 0.4, 0.7,
            0.5, 0.7, 0.5, 0.4, 0.6, 1.2, 0.5,
        ];
        for (index, elapsed_ms) in latencies.into_iter().enumerate() {
            self.live_samples.push(PingSample {
                address: self.host.clone(),
                elapsed_ms: Some(elapsed_ms),
                ttl: Some(128),
                error: None,
            });
            self.sample_times
                .push(format!("14:22:{:02}.123", 14 + index));
        }
        self.progress = self
            .live_samples
            .last()
            .cloned()
            .map(|sample| PingProgress {
                host: self.host.clone(),
                sample,
                sent: 24,
                received: 24,
                min_ms: Some(0.4),
                avg_ms: Some(0.6),
                max_ms: Some(1.2),
            });
    }

    fn render_ping_workspace(
        &self,
        ui: &mut egui::Ui,
        samples: &[PingSample],
        palette: ui::ThemePalette,
    ) {
        let (sent, received, min_ms, avg_ms, max_ms) = self.statistics();
        let loss = if sent == 0 {
            0.0
        } else {
            f64::from(sent.saturating_sub(received)) * 100.0 / f64::from(sent)
        };
        ui::card(ui, |ui| {
            let values = [
                ("已发送", sent.to_string(), palette.success_text),
                ("已接收", received.to_string(), palette.success_text),
                (
                    "丢包",
                    format!("{loss:.1}%"),
                    if loss > 0.0 {
                        palette.danger_text
                    } else {
                        palette.success_text
                    },
                ),
                ("最小", format_latency(min_ms), palette.text),
                ("平均", format_latency(avg_ms), palette.text),
                ("最大", format_latency(max_ms), palette.text),
            ];
            render_statistic_strip(ui, &values);
        });
        ui.add_space(ui::SPACE_4);
        let table_height = (ui.ctx().screen_rect().height() * 0.24).clamp(210.0, 236.0);
        ui::table_card(ui, |ui| {
            let column_widths = ping_table_column_widths(ui.available_width());
            egui::ScrollArea::both()
                .max_height(table_height)
                .min_scrolled_height(table_height)
                .auto_shrink([false, false])
                .stick_to_bottom(self.busy && !self.review_seeded)
                .show(ui, |ui| {
                    render_fixed_table_header(
                        ui,
                        &column_widths,
                        &["序号", "时间", "地址", "字节", "TTL", "延迟", "状态"],
                    );
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (index, sample) in samples.iter().enumerate() {
                        let success = sample.elapsed_ms.is_some();
                        render_fixed_table_row_with_height(
                            ui,
                            &column_widths,
                            index,
                            &[
                                (index + 1).to_string(),
                                self.sample_times
                                    .get(index)
                                    .map_or("-", String::as_str)
                                    .into(),
                                sample.address.clone(),
                                self.payload_size.to_string(),
                                sample.ttl.map_or_else(|| "-".into(), |ttl| ttl.to_string()),
                                format_latency(sample.elapsed_ms),
                                if success {
                                    "已接收".into()
                                } else {
                                    "未收到回显".into()
                                },
                            ],
                            &[
                                palette.text,
                                palette.text,
                                palette.text,
                                palette.text,
                                palette.text,
                                if success {
                                    palette.text
                                } else {
                                    palette.danger_text
                                },
                                if success {
                                    palette.success_text
                                } else {
                                    palette.danger_text
                                },
                            ],
                            32.0,
                        );
                    }
                });
        });
        ui.add_space(ui::SPACE_4);
        ui::card(ui, |ui| {
            ui.label(RichText::new("延迟趋势").strong());
            let values = samples
                .iter()
                .map(|sample| sample.elapsed_ms)
                .collect::<Vec<_>>();
            let chart_labels = if self.sample_times.len() == samples.len() {
                self.sample_times.clone()
            } else {
                (1..=samples.len()).map(|index| index.to_string()).collect()
            };
            let chart_height = (ui.ctx().screen_rect().height() * 0.18).clamp(112.0, 184.0);
            render_line_chart(
                ui,
                &values,
                &chart_labels,
                chart_height,
                palette.accent,
                None,
                4,
                &[],
            );
        });
        ui.add_space(0.0);
        ui.horizontal(|ui| {
            let color = if self.busy {
                palette.success_text
            } else {
                palette.weak
            };
            let (dot_rect, _) =
                ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
            ui.painter().circle_filled(dot_rect.center(), 4.0, color);
            ui.label(if self.busy { "运行中" } else { "已完成" });
            if self.busy {
                ui.label(
                    RichText::new(format!(
                        "下一次探测 {:.1} 秒",
                        self.interval_ms as f32 / 1000.0
                    ))
                    .color(palette.weak),
                );
            }
        });
    }
}

fn ping_table_column_widths(available_width: f32) -> [f32; 7] {
    let total = available_width.max(660.0);
    let fixed = 56.0 + 142.0 + 138.0 + 68.0 + 68.0 + 104.0;
    [
        56.0,
        142.0,
        138.0,
        68.0,
        68.0,
        104.0,
        (total - fixed).max(118.0),
    ]
}

fn render_fixed_table_header(ui: &mut egui::Ui, column_widths: &[f32], labels: &[&str]) {
    let palette = ui::palette_for_ui(ui);
    let total_width = column_widths.iter().sum();
    let (response, painter) =
        ui.allocate_painter(egui::vec2(total_width, 28.0), egui::Sense::hover());
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
    for (index, label) in labels.iter().enumerate() {
        let cell = fixed_table_cell(response.rect, column_widths, index);
        paint_fixed_table_text(
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

fn render_fixed_table_row(
    ui: &mut egui::Ui,
    column_widths: &[f32],
    row_index: usize,
    values: &[String],
    colors: &[egui::Color32],
) {
    render_fixed_table_row_with_height(ui, column_widths, row_index, values, colors, 32.0);
}

fn render_fixed_table_row_with_height(
    ui: &mut egui::Ui,
    column_widths: &[f32],
    row_index: usize,
    values: &[String],
    colors: &[egui::Color32],
    row_height: f32,
) {
    let palette = ui::palette_for_ui(ui);
    let total_width = column_widths.iter().sum();
    let (response, painter) =
        ui.allocate_painter(egui::vec2(total_width, row_height), egui::Sense::hover());
    let fill = if row_index % 2 == 1 {
        ui.visuals().faint_bg_color.gamma_multiply(0.55)
    } else {
        egui::Color32::TRANSPARENT
    };
    painter.rect_filled(response.rect, egui::CornerRadius::same(2), fill);
    painter.rect_stroke(
        response.rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.0_f32, palette.border_subtle),
        egui::StrokeKind::Middle,
    );
    for (index, value) in values.iter().enumerate() {
        let cell = fixed_table_cell(response.rect, column_widths, index);
        paint_fixed_table_text(
            &painter,
            cell,
            value,
            egui::FontId::monospace(12.0),
            colors.get(index).copied().unwrap_or(palette.text),
        );
        ui::show_clipped_text_tooltip(
            ui,
            cell,
            ("fixed-table-cell", row_index, index),
            value,
            egui::FontId::monospace(12.0),
            (cell.width() - 16.0).max(0.0),
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

fn fixed_table_cell(row: egui::Rect, widths: &[f32], index: usize) -> egui::Rect {
    let left = row.left() + widths[..index].iter().sum::<f32>();
    egui::Rect::from_min_size(
        egui::pos2(left, row.top()),
        egui::vec2(widths[index], row.height()),
    )
}

fn paint_fixed_table_text(
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
    fn ui(&mut self, ui: &mut egui::Ui, context: ToolUiContext) -> Vec<AppAction> {
        if context.review_mode && !self.review_seeded {
            self.seed_review();
        }
        let mut actions = Vec::new();
        let palette = ui::palette_for_ui(ui);
        ui::page_heading(ui, "TCP 端口测试", "重复建立原始 TCP 连接，不包含 TLS");
        ui.add_space(ui::SPACE_12);
        ui::card(ui, |ui| {
            ui.set_min_width(ui.available_width());
            let mut submit = false;
            let field_widths = [165.0, 74.0, 174.0, 74.0, 74.0, 74.0, 92.0, 78.0];
            ui::responsive_parameter_row(ui, &field_widths, ui::SPACE_8, |ui| {
                ui::parameter_group(ui, 165.0, |ui| {
                    ui.add_sized(
                        [165.0, 16.0],
                        egui::Label::new(RichText::new("主机").size(12.0).color(palette.weak)),
                    );
                    let input = ui.add_sized(
                        [165.0, ui::CONTROL_HEIGHT],
                        ui::text_input(&mut self.host, "主机名或 IP"),
                    );
                    submit |=
                        input.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                });

                ui::parameter_group(ui, 74.0, |ui| {
                    ui.add_sized(
                        [74.0, 16.0],
                        egui::Label::new(RichText::new("端口").size(12.0).color(palette.weak)),
                    );
                    ui.add_sized(
                        [74.0, ui::CONTROL_HEIGHT],
                        egui::DragValue::new(&mut self.port)
                            .range(1..=65_535)
                            .min_decimals(0),
                    );
                });

                ui::parameter_group(ui, 174.0, |ui| {
                    ui.add_sized(
                        [174.0, 16.0],
                        egui::Label::new(
                            RichText::new("地址类型").size(12.0).color(palette.weak),
                        ),
                    );
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
                                    egui::Button::selectable(self.family == value, value.label()),
                                )
                                .clicked()
                            {
                                self.family = value;
                            }
                        }
                    });
                });

                ui::parameter_group(ui, 74.0, |ui| {
                    ui.add_sized(
                        [74.0, 16.0],
                        egui::Label::new(
                            RichText::new("超时(ms)").size(12.0).color(palette.weak),
                        ),
                    );
                    ui.add_sized(
                        [74.0, ui::CONTROL_HEIGHT],
                        egui::DragValue::new(&mut self.timeout_ms).range(100..=30_000),
                    );
                });

                ui::parameter_group(ui, 74.0, |ui| {
                    ui.add_sized(
                        [74.0, 16.0],
                        egui::Label::new(
                            RichText::new("尝试次数").size(12.0).color(palette.weak),
                        ),
                    );
                    ui.add_sized(
                        [74.0, ui::CONTROL_HEIGHT],
                        egui::DragValue::new(&mut self.attempts).range(1..=100),
                    );
                });

                ui::parameter_group(ui, 74.0, |ui| {
                    ui.add_sized(
                        [74.0, 16.0],
                        egui::Label::new(
                            RichText::new("间隔(ms)").size(12.0).color(palette.weak),
                        ),
                    );
                    ui.add_sized(
                        [74.0, ui::CONTROL_HEIGHT],
                        egui::DragValue::new(&mut self.interval_ms).range(100..=60_000),
                    );
                });

                ui::parameter_group(ui, 92.0, |ui| {
                    ui.allocate_space(egui::vec2(92.0, 16.0));
                    ui.add_enabled_ui(!self.busy, |ui| {
                        submit |= ui::primary_button_sized(
                            ui,
                            "开始测试",
                            [92.0, ui::CONTROL_HEIGHT],
                        )
                        .clicked();
                    });
                });

                ui::parameter_group(ui, 78.0, |ui| {
                    ui.allocate_space(egui::vec2(78.0, 16.0));
                    if ui::danger_outline_button_sized(
                        ui,
                        "停止",
                        [78.0, ui::CONTROL_HEIGHT],
                        self.busy,
                    )
                    .clicked()
                        && self.busy
                    {
                        actions.push(AppAction::StopTcpProbe);
                    }
                });
            });
            ui.add_space(ui::SPACE_8);
            ui.horizontal(|ui| {
                for label in ["原始 TCP", "不含 TLS"] {
                    let (dot_rect, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter()
                        .circle_filled(dot_rect.center(), 4.0, palette.success_text);
                    ui.label(label);
                }
            });
            if submit && !self.busy {
                actions.push(AppAction::RunTcpProbe {
                    host: self.host.clone(),
                    port: self.port,
                    config: self.config(),
                });
            }
        });
        if let Some(Err(error)) = &self.result {
            ui.add_space(ui::SPACE_12);
            state(ui, self.busy, Some(error));
        }
        if !self.progress.is_empty()
            || self
                .result
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .is_some_and(|result| !result.attempts.is_empty())
        {
            ui.add_space(0.0);
            self.render_tcp_workspace(ui, palette);
        } else if self.busy {
            ui.add_space(ui::SPACE_12);
            state(ui, true, None);
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
                config: self.config(),
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
        if busy {
            self.progress.clear();
            self.attempt_times.clear();
        }
        self.busy = busy;
    }
    fn handle_tcp_probe_progress(&mut self, progress: TcpProbeProgress) {
        self.progress.push(progress);
        self.attempt_times.push(local_date_time_millis());
        if self.progress.len() > 500 {
            self.progress.remove(0);
            self.attempt_times.remove(0);
        }
    }
    fn poll_actions(&mut self, _now: Instant) -> Vec<AppAction> {
        Vec::new()
    }
}

impl TcpProbeTool {
    fn config(&self) -> TcpProbeConfig {
        TcpProbeConfig {
            family: self.family,
            timeout_ms: self.timeout_ms.clamp(100, 30_000),
            attempts: self.attempts.clamp(1, 100),
            interval_ms: self.interval_ms.clamp(100, 60_000),
        }
    }

    fn seed_review(&mut self) {
        self.review_seeded = true;
        self.host = "127.0.0.1".into();
        self.port = 8080;
        self.timeout_ms = 3_000;
        self.attempts = 10;
        self.interval_ms = 500;
        self.busy = true;
        let values = [8.7, 9.3, 11.1, 10.2, 3000.0, 11.8, 12.5, 31.4, 9.6, 10.9];
        for (index, elapsed_ms) in values.into_iter().enumerate() {
            let status = if index == 4 { "超时" } else { "成功" };
            self.progress.push(TcpProbeProgress {
                attempt_number: index as u32 + 1,
                attempt: crate::model::TcpProbeAttempt {
                    address: "127.0.0.1:8080".into(),
                    elapsed_ms,
                    status: status.into(),
                },
            });
            let millisecond = if index % 2 == 0 { 123 } else { 623 };
            self.attempt_times.push(format!(
                "2025-05-24 14:31:{:02}.{millisecond:03}",
                22 + index / 2
            ));
        }
    }

    fn render_tcp_workspace(&self, ui: &mut egui::Ui, palette: ui::ThemePalette) {
        let fallback = self.result.as_ref().and_then(|result| result.as_ref().ok());
        let attempts = if self.progress.is_empty() {
            fallback
                .map(|result| {
                    result
                        .attempts
                        .iter()
                        .enumerate()
                        .map(|(index, attempt)| (index as u32 + 1, attempt))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        } else {
            self.progress
                .iter()
                .map(|progress| (progress.attempt_number, &progress.attempt))
                .collect::<Vec<_>>()
        };
        let success = attempts
            .iter()
            .filter(|(_, attempt)| attempt.status == "成功")
            .map(|(_, attempt)| attempt.elapsed_ms)
            .collect::<Vec<_>>();
        let success_count = success.len();
        let success_rate = if attempts.is_empty() {
            0.0
        } else {
            success_count as f64 * 100.0 / attempts.len() as f64
        };
        let average =
            (!success.is_empty()).then(|| success.iter().sum::<f64>() / success.len() as f64);
        let maximum = success.iter().copied().reduce(f64::max);
        ui::card(ui, |ui| {
            let values = [
                ("尝试", attempts.len().to_string(), palette.text),
                ("成功", success_count.to_string(), palette.success_text),
                (
                    "成功率",
                    format!("{success_rate:.0}%"),
                    palette.success_text,
                ),
                ("平均", format_latency(average), palette.text),
                ("最大", format_latency(maximum), palette.text),
            ];
            render_statistic_strip(ui, &values);
        });
        ui.add_space(ui::SPACE_12);
        let table_height = (ui.ctx().screen_rect().height() * 0.30).clamp(280.0, 320.0);
        ui::table_card(ui, |ui| {
            let column_widths = tcp_probe_table_column_widths(ui.available_width());
            egui::ScrollArea::both()
                .max_height(table_height)
                .min_scrolled_height(table_height)
                .auto_shrink([false, false])
                .stick_to_bottom(self.busy && !self.review_seeded)
                .show(ui, |ui| {
                    render_fixed_table_header(
                        ui,
                        &column_widths,
                        &["次数", "开始时间", "地址", "端口", "状态", "延迟", "错误"],
                    );
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for (index, (number, attempt)) in attempts.iter().enumerate() {
                        let succeeded = attempt.status == "成功";
                        let status = if succeeded {
                            "连接成功"
                        } else if attempt.status == "超时" {
                            "连接超时"
                        } else {
                            "连接失败"
                        };
                        render_fixed_table_row_with_height(
                            ui,
                            &column_widths,
                            index,
                            &[
                                number.to_string(),
                                self.attempt_times
                                    .get(index)
                                    .map_or("-", String::as_str)
                                    .into(),
                                tcp_attempt_host(&attempt.address),
                                self.port.to_string(),
                                status.into(),
                                format!("{:.1} ms", attempt.elapsed_ms),
                                if succeeded {
                                    "-".into()
                                } else {
                                    attempt.status.clone()
                                },
                            ],
                            &[
                                palette.text,
                                palette.text,
                                palette.text,
                                palette.text,
                                if succeeded {
                                    palette.success_text
                                } else {
                                    palette.danger_text
                                },
                                palette.text,
                                if succeeded {
                                    palette.weak
                                } else {
                                    palette.danger_text
                                },
                            ],
                            32.0,
                        );
                    }
                });
        });
        ui.add_space(ui::SPACE_12);
        ui::card(ui, |ui| {
            ui.label(RichText::new("延迟趋势 (ms)").strong());
            let values = attempts
                .iter()
                .map(|(_, attempt)| Some(attempt.elapsed_ms.min(50.0)))
                .collect::<Vec<_>>();
            let chart_labels = attempts
                .iter()
                .map(|(number, _)| number.to_string())
                .collect::<Vec<_>>();
            let highlighted_indices = attempts
                .iter()
                .enumerate()
                .filter_map(|(index, (_, attempt))| (attempt.status != "成功").then_some(index))
                .collect::<Vec<_>>();
            render_line_chart(
                ui,
                &values,
                &chart_labels,
                126.0,
                palette.success_text,
                Some(50.0),
                2,
                &highlighted_indices,
            );
        });
    }
}

fn tcp_probe_table_column_widths(available_width: f32) -> [f32; 7] {
    let total = available_width.max(760.0);
    let number = (total * 0.072).max(56.0);
    let started_at = (total * 0.216).max(164.0);
    let address = (total * 0.148).max(112.0);
    let port = (total * 0.118).max(90.0);
    let status = (total * 0.149).max(113.0);
    let latency = (total * 0.139).max(106.0);
    let error = (total - number - started_at - address - port - status - latency).max(96.0);
    [number, started_at, address, port, status, latency, error]
}

fn tcp_attempt_host(address: &str) -> String {
    address
        .parse::<std::net::SocketAddr>()
        .map_or_else(|_| address.to_owned(), |address| address.ip().to_string())
}

#[cfg(test)]
mod tests {
    use eframe::egui;

    use super::{DnsLookupTool, PingTool, TcpProbeTool};
    use crate::tools::{ToolModule, ToolUiContext};

    #[test]
    fn dns_review_page_produces_painted_shapes() {
        let context = egui::Context::default();
        let mut tool = DnsLookupTool::default();
        let output = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| {
                tool.ui(
                    ui,
                    ToolUiContext {
                        review_mode: true,
                        ..ToolUiContext::default()
                    },
                );
            });
        });
        assert!(!output.shapes.is_empty());
    }

    #[test]
    fn network_tool_defaults_match_the_documented_probe_values() {
        let ping = PingTool::default();
        assert_eq!(ping.count, 4);
        assert_eq!(ping.timeout_ms, 1_000);
        assert_eq!(ping.payload_size, 32);
        assert_eq!(ping.interval_ms, 1_000);

        let tcp = TcpProbeTool::default();
        assert_eq!(tcp.port, 443);
        assert_eq!(tcp.timeout_ms, 3_000);
        assert_eq!(tcp.attempts, 3);
        assert_eq!(tcp.interval_ms, 1_000);
    }
}
