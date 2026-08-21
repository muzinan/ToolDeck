//! 文件占用工具模块。
//! 该模块维护查询输入与展示状态，实际 Restart Manager 调用由 Worker 和 Windows 平台层执行。

use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText, TextEdit};

use crate::{
    core::{actions::AppAction, invocation::ToolPayload, worker::TaskResult},
    model::{AppError, FileLockResult},
    tools::{
        ToolModule, ToolUiContext,
        registry::{ToolCategory, ToolDescriptor},
    },
};

#[derive(Default)]
pub struct FileLockTool {
    path: String,
    result: Option<Result<FileLockResult, AppError>>,
    busy: bool,
}

impl ToolModule for FileLockTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: "file-lock",
            name: "文件占用",
            description: "查询哪个进程正在使用文件",
            category: ToolCategory::File,
            icon: "F",
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

    fn ui(&mut self, ui: &mut egui::Ui, _context: ToolUiContext) -> Vec<AppAction> {
        let mut actions = Vec::new();
        heading(
            ui,
            "文件占用",
            "使用 Windows Restart Manager 查找正在占用文件的进程。",
        );
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_sized(
                [ui.available_width() - 90.0, 32.0],
                TextEdit::singleline(&mut self.path).hint_text("输入文件完整路径或直接拖入文件"),
            );
            let query = ui.add_enabled(!self.busy, egui::Button::new("查询"));
            if query.clicked() {
                actions.extend(self.start_query());
            }
        });

        if self.busy {
            ui.add_space(16.0);
            ui.spinner();
            ui.label("正在查询文件占用...");
        }

        if let Some(result) = &self.result {
            ui.add_space(20.0);
            match result {
                Ok(result) if result.processes.is_empty() => empty_state(
                    ui,
                    "没有检测到进程占用该文件",
                    "Restart Manager 未返回任何占用者。",
                ),
                Ok(result) => {
                    ui.label(RichText::new(&result.path).strong());
                    ui.add_space(10.0);
                    ui.label(RichText::new("正在使用").color(Color32::from_rgb(60, 128, 104)));
                    ui.add_space(6.0);
                    for process in &result.processes {
                        ui.group(|ui| {
                            ui.set_min_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(&process.name).strong());
                                    ui.label(format!("PID {}", process.pid));
                                    if let Some(path) = &process.exe_path {
                                        ui.small(path);
                                    }
                                });
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui.button("查看进程").clicked() {
                                            actions.push(AppAction::InspectProcess {
                                                pid: process.pid,
                                            });
                                        }
                                        if ui.small_button("复制 PID").clicked() {
                                            actions
                                                .push(AppAction::CopyText(process.pid.to_string()));
                                        }
                                    },
                                );
                            });
                        });
                        ui.add_space(5.0);
                    }
                }
                Err(error) => error_state(ui, error),
            }
        } else if !self.busy {
            ui.add_space(44.0);
            empty_state(
                ui,
                "选择一个文件开始查询",
                "支持输入路径、拖入文件或从资源管理器右键菜单打开。",
            )
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
        }
    }

    fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }

    fn is_busy(&self) -> bool {
        self.busy
    }
}

impl FileLockTool {
    fn start_query(&mut self) -> Vec<AppAction> {
        let path = self.path.trim();
        if path.is_empty() {
            self.result = Some(Err(AppError::InvalidInput(
                "请输入要查询的文件路径。".into(),
            )));
            return Vec::new();
        }
        self.result = None;
        vec![AppAction::QueryFileLocks {
            path: PathBuf::from(path),
        }]
    }
}

pub(crate) fn heading(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.heading(title);
    ui.label(RichText::new(subtitle).color(ui.visuals().weak_text_color()));
}

pub(crate) fn empty_state(ui: &mut egui::Ui, title: &str, detail: &str) {
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(title).strong());
        ui.add_space(5.0);
        ui.label(RichText::new(detail).color(ui.visuals().weak_text_color()));
    });
}

pub(crate) fn error_state(ui: &mut egui::Ui, error: &AppError) {
    let (title, detail) = error.user_message();
    ui.group(|ui| {
        ui.label(
            RichText::new(title)
                .color(Color32::from_rgb(190, 66, 66))
                .strong(),
        );
        ui.label(detail);
    });
}
