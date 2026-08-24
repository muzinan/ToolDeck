//! 文件占用工具模块。
//! 该模块维护查询输入与展示状态，实际 Restart Manager 调用由 Worker 和 Windows 平台层执行。

use std::path::PathBuf;

use eframe::egui::{self, RichText};

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
            "使用 Windows Restart Manager 查找正在占用文件的进程与句柄。",
        );
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            match ui::action_layout(ui.available_width()) {
                ui::ActionLayout::Horizontal => {
                    let mut submit = false;
                    ui.horizontal(|ui| {
                        let button_width = 86.0;
                        let input_width = (ui.available_width()
                            - button_width * 2.0
                            - ui.spacing().item_spacing.x * 2.0)
                            .max(140.0);
                        let input = ui.add_sized(
                            [input_width, ui::CONTROL_HEIGHT],
                            ui::text_input(
                                &mut self.path,
                                "输入或粘贴文件完整路径，或直接拖拽文件入内",
                            ),
                        );
                        submit |= input.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        if ui::secondary_button_sized(
                            ui,
                            "选择文件",
                            [button_width, ui::CONTROL_HEIGHT],
                        )
                        .clicked()
                        {
                            actions.push(AppAction::PickFileForLocks);
                        }
                        if ui::primary_button_sized(
                            ui,
                            if self.busy {
                                "重新查询"
                            } else {
                                "查询占用"
                            },
                            [button_width, ui::CONTROL_HEIGHT],
                        )
                        .clicked()
                        {
                            submit = true;
                        }
                    });
                    if submit {
                        actions.extend(self.start_query());
                    }
                }
                ui::ActionLayout::Vertical => {
                    let input = ui.add_sized(
                        [ui.available_width(), ui::CONTROL_HEIGHT],
                        ui::text_input(
                            &mut self.path,
                            "输入或粘贴文件完整路径，或直接拖拽文件入内",
                        ),
                    );
                    let submit_from_keyboard =
                        input.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    ui.add_space(ui::SPACE_8);
                    ui.horizontal_wrapped(|ui| {
                        if ui::secondary_button(ui, "选择文件").clicked() {
                            actions.push(AppAction::PickFileForLocks);
                        }
                        if ui::primary_button(
                            ui,
                            if self.busy {
                                "重新查询"
                            } else {
                                "查询占用"
                            },
                        )
                        .clicked()
                            || submit_from_keyboard
                        {
                            actions.extend(self.start_query());
                        }
                    });
                }
            };
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
                    render_file_result_header(ui, result, &mut actions);
                    ui.add_space(ui::SPACE_12);
                    empty_state(
                        ui,
                        "未检测到文件被任何进程占用",
                        "Restart Manager 未返回任何占用者，文件当前可被安全移动、编辑或删除。",
                    );
                }
                Ok(result) => {
                    render_file_result_header(ui, result, &mut actions);
                    ui.add_space(ui::SPACE_16);
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("检测到 {} 个占用进程", result.processes.len()))
                                .color(ui::danger_text(ui))
                                .strong()
                                .size(15.0),
                        );
                    });
                    ui.add_space(ui::SPACE_8);
                    for process in &result.processes {
                        ui::card(ui, |ui| {
                            let render_actions = |ui: &mut egui::Ui| {
                                if ui::small_action_button(ui, "查看进程").clicked() {
                                    actions.push(AppAction::InvokeTool(ToolInvocation::process(
                                        process.pid,
                                    )));
                                }
                                if ui::small_action_button(ui, "复制 PID").clicked() {
                                    actions.push(AppAction::CopyText(process.pid.to_string()));
                                }
                                if let Some(path) = &process.exe_path {
                                    if ui::small_action_button(ui, "复制路径")
                                        .on_hover_text("复制进程可执行文件完整路径")
                                        .clicked()
                                    {
                                        actions.push(AppAction::CopyText(path.clone()));
                                    }
                                    if ui::small_action_button(ui, "定位文件")
                                        .on_hover_text("在资源管理器中选中进程可执行文件")
                                        .clicked()
                                    {
                                        actions
                                            .push(AppAction::OpenFileLocation(PathBuf::from(path)));
                                    }
                                }
                            };
                            let render_details =
                                |ui: &mut egui::Ui| {
                                    ui.horizontal(|ui| {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(&process.name).strong().size(15.0),
                                            )
                                            .wrap(),
                                        )
                                        .on_hover_text(&process.name);
                                        let palette = ui::palette_for_ui(ui);
                                        ui::badge(
                                            ui,
                                            &format!("PID: {}", process.pid),
                                            palette.accent,
                                            palette.accent.gamma_multiply(
                                                if ui.visuals().dark_mode { 0.22 } else { 0.12 },
                                            ),
                                        );
                                    });
                                    if let Some(path) = &process.exe_path {
                                        ui.add_space(2.0);
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(path)
                                                    .monospace()
                                                    .small()
                                                    .color(ui.visuals().weak_text_color()),
                                            )
                                            .wrap(),
                                        )
                                        .on_hover_text(path);
                                    }
                                };
                            match ui::action_layout(ui.available_width()) {
                                ui::ActionLayout::Horizontal => {
                                    let action_width = 320.0_f32.min(ui.available_width() * 0.55);
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
                                            egui::Layout::top_down(egui::Align::Max),
                                            |ui| {
                                                ui.horizontal_wrapped(render_actions);
                                            },
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
            ui.add_space(36.0);
            empty_state(
                ui,
                "选择或拖入文件开始查询",
                "支持直接输入路径、点击“选择文件”或从资源管理器右键菜单快捷打开。",
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

fn render_file_result_header(
    ui: &mut egui::Ui,
    result: &FileLockResult,
    actions: &mut Vec<AppAction>,
) {
    ui::card(ui, |ui| {
        ui.label(RichText::new("目标文件").strong().size(14.5));
        ui.add_space(ui::SPACE_4);
        ui.add(
            egui::Label::new(
                RichText::new(&result.path)
                    .monospace()
                    .color(ui::accent(ui)),
            )
            .wrap(),
        )
        .on_hover_text(&result.path);
        ui.add_space(ui::SPACE_8);
        ui.horizontal_wrapped(|ui| {
            if ui::small_action_button(ui, "复制路径").clicked() {
                actions.push(AppAction::CopyText(result.path.clone()));
            }
            if ui::small_action_button(ui, "在资源管理器中定位").clicked() {
                actions.push(AppAction::OpenFileLocation(PathBuf::from(&result.path)));
            }
            if ui::small_action_button(ui, "复制完整报告").clicked() {
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
            }],
        });
        assert!(report.contains(r"C:\测试\data.db"));
        assert!(report.contains("工具😀.exe (PID 42)"));
        assert!(report.contains(r"C:\Apps\sample.exe"));
    }
}
