//! 文件占用工具模块。
//! 该模块维护查询输入与展示状态，实际 Restart Manager 调用由 Worker 和 Windows 平台层执行。

use std::path::PathBuf;

use eframe::egui::{self, RichText, TextEdit};

use crate::{
    core::{
        actions::AppAction,
        invocation::{ToolInvocation, ToolPayload},
        worker::TaskResult,
    },
    model::{AppError, FileLockResult},
    tools::{
        ToolModule, ToolUiContext,
        registry::{ToolCategory, ToolDescriptor, ToolIcon},
    },
    ui,
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

    fn ui(&mut self, ui: &mut egui::Ui, _context: ToolUiContext) -> Vec<AppAction> {
        let mut actions = Vec::new();
        heading(
            ui,
            "文件占用",
            "使用 Windows Restart Manager 查找正在占用文件的进程。",
        );
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            let render_controls = |ui: &mut egui::Ui| {
                ui.add_sized(
                    [
                        if matches!(
                            ui::action_layout(ui.available_width()),
                            ui::ActionLayout::Horizontal
                        ) {
                            (ui.available_width() - 88.0).max(120.0)
                        } else {
                            ui.available_width()
                        },
                        34.0,
                    ],
                    TextEdit::singleline(&mut self.path)
                        .hint_text("输入文件完整路径或直接拖入文件"),
                );
                let query = ui
                    .add_enabled_ui(!self.busy, |ui| ui::primary_button(ui, "查询"))
                    .inner;
                if query.clicked() {
                    actions.extend(self.start_query());
                }
            };
            match ui::action_layout(ui.available_width()) {
                ui::ActionLayout::Horizontal => ui.horizontal(render_controls),
                ui::ActionLayout::Vertical => ui.vertical(render_controls),
            };
        });

        if self.busy {
            ui.add_space(ui::SPACE_16);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("正在查询文件占用...");
            });
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
                    ui::card(ui, |ui| {
                        ui.label(RichText::new("查询文件").strong());
                        ui.add_space(ui::SPACE_4);
                        ui.add(egui::Label::new(RichText::new(&result.path).monospace()).wrap())
                            .on_hover_text(&result.path);
                    });
                    ui.add_space(ui::SPACE_12);
                    ui.label(
                        RichText::new("正在使用")
                            .color(ui::success_text(ui))
                            .strong(),
                    );
                    ui.add_space(ui::SPACE_8);
                    for process in &result.processes {
                        ui::card(ui, |ui| {
                            let render_actions = |ui: &mut egui::Ui| {
                                if ui::primary_button(ui, "查看进程").clicked() {
                                    actions.push(AppAction::InvokeTool(ToolInvocation::process(
                                        process.pid,
                                    )));
                                }
                                if ui.small_button("复制 PID").clicked() {
                                    actions.push(AppAction::CopyText(process.pid.to_string()));
                                }
                            };
                            let render_details = |ui: &mut egui::Ui| {
                                ui.add(
                                    egui::Label::new(RichText::new(&process.name).strong()).wrap(),
                                )
                                .on_hover_text(&process.name);
                                ui.label(format!("PID {}", process.pid));
                                if let Some(path) = &process.exe_path {
                                    ui.add(egui::Label::new(RichText::new(path).small()).wrap())
                                        .on_hover_text(path);
                                }
                            };
                            match ui::action_layout(ui.available_width()) {
                                ui::ActionLayout::Horizontal => {
                                    let action_width = 164.0;
                                    let info_width =
                                        (ui.available_width() - action_width - ui::SPACE_8)
                                            .max(160.0);
                                    ui.horizontal(|ui| {
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(info_width, 0.0),
                                            egui::Layout::top_down(egui::Align::Min),
                                            render_details,
                                        );
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(action_width, 0.0),
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            render_actions,
                                        );
                                    });
                                }
                                ui::ActionLayout::Vertical => {
                                    ui.vertical(render_details);
                                    ui.add_space(ui::SPACE_8);
                                    ui.horizontal_wrapped(render_actions);
                                }
                            }
                        });
                        ui.add_space(ui::SPACE_8);
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
    ui::page_heading(ui, title, subtitle);
}

pub(crate) fn empty_state(ui: &mut egui::Ui, title: &str, detail: &str) {
    ui::state_card(ui, title, detail, ui::accent(ui));
}

pub(crate) fn error_state(ui: &mut egui::Ui, error: &AppError) {
    let (title, detail) = error.user_message();
    ui::state_card(ui, title, &detail, ui::danger_text(ui));
}
