//! 文件占用工具模块。
//! 该模块维护查询输入与展示状态，实际 Restart Manager 调用由 Worker 和 Windows 平台层执行。

use std::{
    collections::VecDeque,
    path::PathBuf,
    time::{Duration, Instant},
};

use eframe::egui::{self, RichText};

use crate::{
    core::{
        actions::AppAction,
        invocation::{ToolInvocation, ToolPayload},
        worker::TaskResult,
    },
    model::{AppError, FileLockResult, ProcessSummary},
    tools::{
        ToolModule, ToolUiContext,
        registry::{ToolCategory, ToolDescriptor, ToolIcon},
    },
    ui,
};

pub struct FileLockTool {
    path: String,
    filter: String,
    auto_refresh: bool,
    refresh_seconds: u64,
    next_refresh: Option<Instant>,
    recent_paths: VecDeque<String>,
    selected_pid: Option<u32>,
    review_seeded: bool,
    result: Option<Result<FileLockResult, AppError>>,
    busy: bool,
}

impl Default for FileLockTool {
    fn default() -> Self {
        Self {
            path: String::new(),
            filter: String::new(),
            auto_refresh: false,
            refresh_seconds: 5,
            next_refresh: None,
            recent_paths: VecDeque::new(),
            selected_pid: None,
            review_seeded: false,
            result: None,
            busy: false,
        }
    }
}

impl ToolModule for FileLockTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: "file-lock",
            name: "文件占用",
            description: "查询哪个进程正在使用文件",
            category: ToolCategory::Diagnostic,
            icon: ToolIcon::FileLock,
            keywords: &[
                "file",
                "lock",
                "handle",
                "restart manager",
                "文件",
                "占用",
                "锁定",
            ],
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, context: ToolUiContext) -> Vec<AppAction> {
        if context.review_mode && !self.review_seeded {
            self.seed_review();
        }
        let mut actions = Vec::new();
        heading(ui, "文件占用", "查找正在占用文件或目录的进程");
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            let mut submit = false;
            ui.horizontal_wrapped(|ui| {
                ui.label("最近路径");
                egui::ComboBox::from_id_salt("file-lock-recent-paths")
                    .width(76.0)
                    .selected_text("会话")
                    .show_ui(ui, |ui| {
                        for path in &self.recent_paths {
                            if ui.selectable_label(false, path).clicked() {
                                self.path.clone_from(path);
                            }
                        }
                    });
                let input = ui.add_sized(
                    [320.0_f32.min(ui.available_width()), ui::CONTROL_HEIGHT],
                    ui::text_input(&mut self.path, "输入或粘贴文件完整路径"),
                );
                submit |=
                    input.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                submit |= ui::primary_button_sized(
                    ui,
                    if self.busy { "重新查询" } else { "查询" },
                    [66.0, ui::CONTROL_HEIGHT],
                )
                .clicked();
                ui.label("刷新间隔");
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (label, seconds) in [
                        ("关闭", 0),
                        ("1秒", 1),
                        ("2秒", 2),
                        ("5秒", 5),
                        ("10秒", 10),
                    ] {
                        let selected = if seconds == 0 {
                            !self.auto_refresh
                        } else {
                            self.auto_refresh && self.refresh_seconds == seconds
                        };
                        if ui
                            .add_sized(
                                [46.0, ui::CONTROL_HEIGHT],
                                egui::Button::selectable(selected, label),
                            )
                            .clicked()
                        {
                            self.auto_refresh = seconds != 0;
                            if seconds != 0 {
                                self.refresh_seconds = seconds;
                                self.next_refresh =
                                    Some(Instant::now() + Duration::from_secs(seconds));
                            } else {
                                self.next_refresh = None;
                            }
                        }
                    }
                });
                if ui::secondary_button(ui, "刷新").clicked() {
                    submit = true;
                }
            });
            if submit {
                actions.extend(self.start_query());
            }
            ui.add_space(ui::SPACE_8);
            let filter_options = self
                .result
                .as_ref()
                .and_then(|result| result.as_ref().ok())
                .map(|result| {
                    result
                        .processes
                        .iter()
                        .map(|process| process.name.clone())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                ui.label("筛选进程");
                let selected_filter = if self.filter.is_empty() {
                    "所有进程".to_owned()
                } else {
                    self.filter.clone()
                };
                egui::ComboBox::from_id_salt("file-lock-process-filter")
                    .width(140.0)
                    .selected_text(selected_filter)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.filter, String::new(), "所有进程");
                        for name in &filter_options {
                            ui.selectable_value(&mut self.filter, name.clone(), name);
                        }
                    });
                let count = self
                    .result
                    .as_ref()
                    .and_then(|result| result.as_ref().ok())
                    .map_or(0, |result| result.processes.len());
                ui.label(format!("找到 {count} 个占用进程"));
            });
        });

        if self.busy {
            ui.add_space(ui::SPACE_16);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("正在通过 Restart Manager 检索占用进程...").size(13.5));
            });
        }

        if let Some(result) = &self.result {
            ui.add_space(ui::SPACE_16);
            match result {
                Ok(result) if result.processes.is_empty() => {
                    empty_state(ui, "未检测到占用进程", "当前文件没有可见的占用者。");
                }
                Ok(result) => {
                    let filter = self.filter.trim().to_lowercase();
                    let filtered = result
                        .processes
                        .iter()
                        .filter(|process| process_matches(process, &filter))
                        .collect::<Vec<_>>();
                    let mut selected_pid = self
                        .selected_pid
                        .filter(|pid| result.processes.iter().any(|process| process.pid == *pid));
                    if selected_pid.is_none() {
                        selected_pid = filtered.first().map(|process| process.pid);
                    }
                    render_file_process_table(ui, &filtered, &result.path, &mut selected_pid);
                    self.selected_pid = selected_pid;

                    if let Some(process) = result
                        .processes
                        .iter()
                        .find(|process| Some(process.pid) == self.selected_pid)
                    {
                        ui.add_space(ui::SPACE_12);
                        render_selected_process(ui, process, result, &mut actions);
                    }
                }
                Err(error) => error_state(ui, error),
            }
        } else if !self.busy {
            ui.add_space(36.0);
            empty_state(ui, "输入文件路径开始查询", "也可以直接拖入文件。");
        }
        actions
    }

    fn handle_invocation(&mut self, payload: ToolPayload) -> Vec<AppAction> {
        match payload {
            ToolPayload::FilePath { path } => {
                self.path = path.display().to_string();
                self.result = None;
                self.start_query()
            }
            _ => Vec::new(),
        }
    }

    fn handle_task_result(&mut self, result: TaskResult) {
        if let TaskResult::FileLocks(result) = result {
            self.busy = false;
            self.result = Some(result);
            self.next_refresh = self
                .auto_refresh
                .then(|| Instant::now() + Duration::from_secs(self.refresh_seconds));
        }
    }

    fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    fn poll_actions(&mut self, now: Instant) -> Vec<AppAction> {
        if self.auto_refresh
            && !self.busy
            && self.next_refresh.is_some_and(|next| now >= next)
            && !self.path.trim().is_empty()
        {
            self.next_refresh = Some(now + Duration::from_secs(self.refresh_seconds));
            return vec![AppAction::QueryFileLocks {
                path: PathBuf::from(self.path.trim()),
            }];
        }
        Vec::new()
    }
}

impl FileLockTool {
    fn seed_review(&mut self) {
        self.review_seeded = true;
        self.path = r"C:\Windows\System32\drivers\etc\hosts".into();
        self.recent_paths.push_back(self.path.clone());
        self.selected_pid = Some(12345);
        self.result = Some(Ok(FileLockResult {
            path: self.path.clone(),
            processes: vec![
                ProcessSummary {
                    pid: 12345,
                    name: "notepad.exe".into(),
                    exe_path: Some(r"C:\Windows\System32\notepad.exe".into()),
                    owner: Some("NT AUTHORITY\\SYSTEM".into()),
                    started_at: Some("2025-05-20 14:20:18".into()),
                    ..Default::default()
                },
                ProcessSummary {
                    pid: 6789,
                    name: "svchost.exe".into(),
                    exe_path: Some(r"C:\Windows\System32\svchost.exe".into()),
                    owner: Some("NT AUTHORITY\\LOCAL SERVICE".into()),
                    ..Default::default()
                },
                ProcessSummary {
                    pid: 9876,
                    name: "SearchIndexer.exe".into(),
                    exe_path: Some(r"C:\Windows\System32\SearchIndexer.exe".into()),
                    owner: Some("NT AUTHORITY\\SYSTEM".into()),
                    ..Default::default()
                },
            ],
        }));
    }

    fn start_query(&mut self) -> Vec<AppAction> {
        let path = self.path.trim();
        if path.is_empty() {
            self.result = Some(Err(AppError::InvalidInput(
                "请输入要查询的文件路径。".into(),
            )));
            return Vec::new();
        }
        self.result = None;
        self.recent_paths.retain(|existing| existing != path);
        self.recent_paths.push_front(path.to_owned());
        self.recent_paths.truncate(8);
        vec![AppAction::QueryFileLocks {
            path: PathBuf::from(path),
        }]
    }
}

fn file_lock_report(result: &FileLockResult) -> String {
    let mut report = format!("Windows Toolbox 文件占用报告\n文件：{}\n", result.path);
    if result.processes.is_empty() {
        report.push_str("占用进程：无\n");
        return report;
    }
    report.push_str(&format!("占用进程：{} 个\n", result.processes.len()));
    for process in &result.processes {
        report.push_str(&format!("- {} (PID {})", process.name, process.pid));
        if let Some(path) = &process.exe_path {
            report.push_str(&format!("\n  路径：{path}"));
        }
        report.push('\n');
    }
    report
}

fn process_matches(process: &ProcessSummary, filter: &str) -> bool {
    filter.is_empty()
        || process.name.to_lowercase().contains(filter)
        || process.pid.to_string().contains(filter)
        || process
            .exe_path
            .as_deref()
            .is_some_and(|path| path.to_lowercase().contains(filter))
}

fn render_file_process_table(
    ui: &mut egui::Ui,
    processes: &[&ProcessSummary],
    target_path: &str,
    selected_pid: &mut Option<u32>,
) {
    ui::table_card(ui, |ui| {
        let available = ui.available_width();
        let process_width = (available * 0.16).max(150.0);
        let pid_width = 78.0;
        let kind_width = 74.0;
        let access_width = 88.0;
        let owner_width = (available * 0.21).max(200.0);
        let path_width = (available
            - process_width
            - pid_width
            - kind_width
            - access_width
            - owner_width)
            .max(260.0);
        let widths = [
            process_width,
            pid_width,
            kind_width,
            access_width,
            owner_width,
            path_width,
        ];
        let total_width = widths.iter().sum();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (label, width) in [
                ("进程", widths[0]),
                ("PID", widths[1]),
                ("类型", widths[2]),
                ("访问模式", widths[3]),
                ("用户", widths[4]),
                ("路径", widths[5]),
            ] {
                ui.add_sized(
                    [width, 30.0],
                    egui::Label::new(RichText::new(label).strong()),
                );
            }
        });
        ui.separator();
        egui::ScrollArea::both()
            .max_height(270.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for process in processes {
                    let selected = *selected_pid == Some(process.pid);
                    let row = egui::Frame::new()
                        .fill(if selected {
                            ui::accent(ui).gamma_multiply(0.22)
                        } else {
                            egui::Color32::TRANSPARENT
                        })
                        .show(ui, |ui| {
                            ui.set_width(total_width);
                            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                            ui.horizontal(|ui| {
                                if ui
                                    .add_sized(
                                        [widths[0], 38.0],
                                        egui::Label::new(RichText::new(&process.name))
                                            .sense(egui::Sense::click()),
                                    )
                                    .clicked()
                                {
                                    *selected_pid = Some(process.pid);
                                }
                                ui.add_sized(
                                    [widths[1], 38.0],
                                    egui::Label::new(
                                        RichText::new(process.pid.to_string()).monospace(),
                                    ),
                                );
                                ui.add_sized([widths[2], 38.0], egui::Label::new("文件"));
                                ui.add_sized([widths[3], 38.0], egui::Label::new("读"));
                                let owner = process.owner.as_deref().unwrap_or("—");
                                ui.add_sized(
                                    [widths[4], 38.0],
                                    egui::Label::new(RichText::new(owner).monospace()).truncate(),
                                )
                                .on_hover_text(owner);
                                ui.add_sized(
                                    [widths[5], 38.0],
                                    egui::Label::new(RichText::new(target_path).monospace()).truncate(),
                                )
                                .on_hover_text(target_path);
                            });
                        });
                    if row.response.clicked() {
                        *selected_pid = Some(process.pid);
                    }
                    ui.separator();
                }
            });
    });
}

fn render_selected_process(
    ui: &mut egui::Ui,
    process: &ProcessSummary,
    result: &FileLockResult,
    actions: &mut Vec<AppAction>,
) {
    ui::card(ui, |ui| {
        ui.label(RichText::new("进程详情").strong().size(14.5));
        ui.add_space(ui::SPACE_8);
        egui::Grid::new("file-lock-process-detail")
            .num_columns(2)
            .spacing([ui::SPACE_16, ui::SPACE_8])
            .show(ui, |ui| {
                ui.label("进程名称：");
                ui.label(RichText::new(&process.name).strong());
                ui.end_row();
                ui.label("PID：");
                ui.label(RichText::new(process.pid.to_string()).monospace());
                ui.end_row();
                ui.label("可执行文件：");
                let path = process.exe_path.as_deref().unwrap_or("无法读取");
                ui.add(egui::Label::new(RichText::new(path).monospace()).truncate())
                    .on_hover_text(path);
                ui.end_row();
                ui.label("启动时间：");
                ui.label(
                    RichText::new(process.started_at.as_deref().unwrap_or("无法读取"))
                        .monospace(),
                );
                ui.end_row();
                ui.label("用户：");
                ui.label(
                    RichText::new(process.owner.as_deref().unwrap_or("无法读取")).monospace(),
                );
                ui.end_row();
            });
        ui.add_space(ui::SPACE_12);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui::danger_button(ui, "结束进程").clicked() {
                actions.push(AppAction::RequestTerminateProcess(process.clone()));
            }
            if ui::secondary_button(ui, "跳转到进程").clicked() {
                actions.push(AppAction::InvokeTool(ToolInvocation::process(process.pid)));
            }
            if let Some(path) = &process.exe_path
                && ui::secondary_button(ui, "打开位置").clicked()
            {
                actions.push(AppAction::OpenFileLocation(PathBuf::from(path)));
            }
            if ui::secondary_button(ui, "复制").clicked() {
                actions.push(AppAction::CopyText(file_lock_report(result)));
            }
        });
    });
}

pub(crate) fn heading(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui::page_heading(ui, title, subtitle);
}

pub(crate) fn empty_state(ui: &mut egui::Ui, title: &str, detail: &str) {
    ui::state_card(ui, title, detail, ui::accent(ui));
}

pub(crate) fn error_state(ui: &mut egui::Ui, error: &AppError) {
    let (title, detail) = error.user_message();
    ui::state_card(ui, title, &detail, ui::danger_text(ui));
}

#[cfg(test)]
mod tests {
    use crate::model::{FileLockResult, ProcessSummary};

    use super::file_lock_report;

    #[test]
    fn full_report_contains_file_and_process_details() {
        let report = file_lock_report(&FileLockResult {
            path: r"C:\测试\data.db".into(),
            processes: vec![ProcessSummary {
                pid: 42,
                name: "工具😀.exe".into(),
                exe_path: Some(r"C:\Apps\sample.exe".into()),
                ..Default::default()
            }],
        });
        assert!(report.contains(r"C:\测试\data.db"));
        assert!(report.contains("工具😀.exe (PID 42)"));
        assert!(report.contains(r"C:\Apps\sample.exe"));
    }
}
