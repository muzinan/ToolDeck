//! 进程关系工具模块。
//! 使用父进程链解释进程来源；命令行字段在当前版本明确标注为未读取，避免依赖未公开 NT API。

use std::path::PathBuf;

use eframe::egui::{self, RichText, TextEdit};

use crate::{
    core::{
        actions::AppAction,
        invocation::{ToolInvocation, ToolPayload},
        worker::TaskResult,
    },
    model::{AppError, ProcessInfo, ProcessSummary},
    tools::{
        ToolModule, ToolUiContext,
        file_lock::{empty_state, error_state, heading},
        registry::{ToolCategory, ToolDescriptor, ToolIcon},
    },
    ui,
};

#[derive(Default)]
pub struct ProcessInspectorTool {
    pid_input: String,
    result: Option<Result<ProcessInfo, AppError>>,
    busy: bool,
}

impl ToolModule for ProcessInspectorTool {
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor {
            id: "process-inspector",
            name: "进程关系",
            description: "查看进程详情、父进程链和启动来源",
            category: ToolCategory::System,
            icon: ToolIcon::Process,
            keywords: &[
                "process",
                "pid",
                "tree",
                "parent",
                "command",
                "进程",
                "父进程",
                "进程树",
            ],
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _context: ToolUiContext) -> Vec<AppAction> {
        let mut actions = Vec::new();
        heading(
            ui,
            "进程关系",
            "查看进程详情与完整父进程链，理解它从哪里启动。",
        );
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            match ui::action_layout(ui.available_width()) {
                ui::ActionLayout::Horizontal => {
                    let mut submit = false;
                    ui.horizontal(|ui| {
                        let input = ui.add_sized(
                            [(ui.available_width() - 104.0).max(160.0), 32.0],
                            TextEdit::singleline(&mut self.pid_input).hint_text("输入 PID"),
                        );
                        submit |= input.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        submit |= ui::primary_button(
                            ui,
                            if self.busy {
                                "再次查看"
                            } else {
                                "查看进程"
                            },
                        )
                        .clicked();
                    });
                    if submit {
                        actions.extend(self.start_query());
                    }
                }
                ui::ActionLayout::Vertical => {
                    let input = ui.add_sized(
                        [ui.available_width(), 32.0],
                        TextEdit::singleline(&mut self.pid_input).hint_text("输入 PID"),
                    );
                    let submit =
                        input.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    if ui::primary_button(
                        ui,
                        if self.busy {
                            "再次查看"
                        } else {
                            "查看进程"
                        },
                    )
                    .clicked()
                        || submit
                    {
                        actions.extend(self.start_query());
                    }
                }
            };
        });

        if self.busy {
            ui.add_space(ui::SPACE_16);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("正在读取进程信息...");
            });
        }

        if let Some(result) = &self.result {
            ui.add_space(ui::SPACE_24);
            match result {
                Ok(process) => self.render_process(ui, process, &mut actions),
                Err(error) => error_state(ui, error),
            }
        } else if !self.busy {
            ui.add_space(44.0);
            empty_state(
                ui,
                "输入 PID 查看进程来源",
                "端口和文件占用页面中的进程名称也可以直接跳转到这里。",
            )
        }
        actions
    }

    fn handle_invocation(&mut self, payload: ToolPayload) -> Vec<AppAction> {
        match payload {
            ToolPayload::Process { pid } => {
                self.pid_input = pid.to_string();
                self.result = None;
                vec![AppAction::InspectProcess { pid }]
            }
            _ => Vec::new(),
        }
    }

    fn handle_task_result(&mut self, result: TaskResult) {
        match result {
            TaskResult::Process(result) => {
                self.busy = false;
                self.result = Some(result);
            }
            TaskResult::ProcessTerminated(result) => {
                self.busy = false;
                match result {
                    Ok(pid) => {
                        self.result = Some(Err(AppError::ProcessExited(pid)));
                    }
                    Err(error) => self.result = Some(Err(error)),
                }
            }
            _ => {}
        }
    }

    fn set_busy(&mut self, busy: bool) {
        self.busy = busy;
    }
}

impl ProcessInspectorTool {
    fn start_query(&mut self) -> Vec<AppAction> {
        match self.pid_input.trim().parse::<u32>() {
            Ok(pid) if pid > 0 => vec![AppAction::InspectProcess { pid }],
            _ => {
                self.result = Some(Err(AppError::InvalidInput("PID 必须是正整数。".into())));
                Vec::new()
            }
        }
    }

    fn render_process(
        &self,
        ui: &mut egui::Ui,
        process: &ProcessInfo,
        actions: &mut Vec<AppAction>,
    ) {
        ui::card(ui, |ui| {
            let render_actions = |ui: &mut egui::Ui| {
                if ui::primary_button(ui, "刷新").clicked() {
                    actions.push(AppAction::InspectProcess { pid: process.pid });
                }
                if ui.small_button("查看端口").clicked() {
                    actions.push(AppAction::InvokeTool(ToolInvocation::ports_for_process(
                        process.pid,
                    )));
                }
                if ui.small_button("复制报告").clicked() {
                    actions.push(AppAction::CopyText(process_report(process)));
                }
                if ui::danger_button(ui, "结束进程").clicked() {
                    actions.push(AppAction::RequestTerminateProcess(process.summary()));
                }
                if ui.small_button("复制 PID").clicked() {
                    actions.push(AppAction::CopyText(process.pid.to_string()));
                }
            };
            let render_summary = |ui: &mut egui::Ui| {
                ui.add(egui::Label::new(RichText::new(&process.name).heading().strong()).wrap())
                    .on_hover_text(&process.name);
                ui.label(format!("PID {}", process.pid));
            };
            match ui::action_layout(ui.available_width()) {
                ui::ActionLayout::Horizontal => {
                    let action_width = 420.0_f32.min(ui.available_width() * 0.58);
                    let summary_width =
                        (ui.available_width() - action_width - ui::SPACE_8).max(160.0);
                    ui.horizontal(|ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(summary_width, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            render_summary,
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
                    ui.vertical(render_summary);
                    ui.add_space(ui::SPACE_8);
                    ui.horizontal_wrapped(render_actions);
                }
            }
        });
        ui.add_space(ui::SPACE_12);
        ui::card(ui, |ui| {
            ui.strong("路径");
            let path = process
                .exe_path
                .as_deref()
                .unwrap_or("无法读取，可能需要管理员权限");
            match ui::action_layout(ui.available_width()) {
                ui::ActionLayout::Horizontal if process.exe_path.is_some() => {
                    let action_width = 214.0;
                    let text_width = (ui.available_width() - action_width - ui::SPACE_8).max(160.0);
                    ui.horizontal(|ui| {
                        ui.allocate_ui_with_layout(
                            egui::vec2(text_width, 0.0),
                            egui::Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.add(egui::Label::new(RichText::new(path).monospace()).wrap())
                                    .on_hover_text(path);
                            },
                        );
                        ui.allocate_ui_with_layout(
                            egui::vec2(action_width, 0.0),
                            egui::Layout::top_down(egui::Align::Max),
                            |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    if ui
                                        .small_button("复制路径")
                                        .on_hover_text("复制当前进程可执行文件完整路径")
                                        .clicked()
                                    {
                                        actions.push(AppAction::CopyText(path.to_owned()));
                                    }
                                    if ui
                                        .small_button("打开所在位置")
                                        .on_hover_text("在资源管理器中选中当前进程可执行文件")
                                        .clicked()
                                    {
                                        actions
                                            .push(AppAction::OpenFileLocation(PathBuf::from(path)));
                                    }
                                });
                            },
                        );
                    });
                }
                _ => {
                    ui.add(egui::Label::new(RichText::new(path).monospace()).wrap())
                        .on_hover_text(path);
                    if process.exe_path.is_some() {
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .small_button("复制路径")
                                .on_hover_text("复制当前进程可执行文件完整路径")
                                .clicked()
                            {
                                actions.push(AppAction::CopyText(path.to_owned()));
                            }
                            if ui
                                .small_button("打开所在位置")
                                .on_hover_text("在资源管理器中选中当前进程可执行文件")
                                .clicked()
                            {
                                actions.push(AppAction::OpenFileLocation(PathBuf::from(path)));
                            }
                        });
                    }
                }
            }
            ui.add_space(ui::SPACE_12);
            ui.strong("命令行");
            let command_line = process
                .command_line
                .as_deref()
                .unwrap_or("当前版本未使用未公开 API 读取命令行");
            ui.add(egui::Label::new(RichText::new(command_line).monospace()).wrap())
                .on_hover_text(command_line);
            ui.add_space(ui::SPACE_12);
            ui.strong("启动时间");
            ui.label(process.started_at.as_deref().unwrap_or("无法读取"));
            ui.add_space(ui::SPACE_12);
            ui.strong("父进程 PID");
            ui.label(
                process
                    .parent_pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "无".into()),
            );
        });
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            ui.label(RichText::new("启动来源").strong());
            ui.add_space(ui::SPACE_8);
            if process.parent_chain.is_empty() {
                ui.label("父进程关系不可用。");
            }
            for (index, ancestor) in process.parent_chain.iter().enumerate() {
                render_related_process(ui, ancestor, index, actions);
                if index + 1 < process.parent_chain.len() {
                    ui.add_space(ui::SPACE_8);
                }
            }
        });
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            ui.label(RichText::new("直接子进程").strong());
            ui.add_space(ui::SPACE_8);
            if process.children.is_empty() {
                ui.label("当前快照中没有直接子进程。");
            }
            for (index, child) in process.children.iter().enumerate() {
                render_related_process(ui, child, 0, actions);
                if index + 1 < process.children.len() {
                    ui.add_space(ui::SPACE_8);
                }
            }
        });
    }
}

fn render_related_process(
    ui: &mut egui::Ui,
    process: &ProcessSummary,
    depth: usize,
    actions: &mut Vec<AppAction>,
) {
    ui.horizontal(|ui| {
        // 深层链最多缩进八级，其余层级仍通过顺序表达，避免把正文挤出可用宽度。
        ui.add_space((depth.min(8) as f32) * 12.0);
        if depth > 0 {
            ui.monospace("└─");
        }
        ui.vertical(|ui| {
            let label = format!("{}  (PID {})", process.name, process.pid);
            let response = ui.add(
                egui::Label::new(RichText::new(&label).color(ui::accent(ui)))
                    .wrap()
                    .sense(egui::Sense::click()),
            );
            response.clone().on_hover_text(&label);
            if response.clicked() {
                response.request_focus();
            }
            if response.has_focus() {
                ui.painter().rect_stroke(
                    response.rect.expand(2.0),
                    egui::CornerRadius::same(4),
                    egui::Stroke::new(1.5_f32, ui::accent(ui)),
                    egui::StrokeKind::Middle,
                );
            }
            if response.clicked()
                || (response.has_focus()
                    && ui.input(|input| {
                        input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
                    }))
            {
                actions.push(AppAction::InspectProcess { pid: process.pid });
            }
            if let Some(path) = &process.exe_path {
                ui.add(egui::Label::new(RichText::new(path).monospace().small()).wrap())
                    .on_hover_text(path);
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .small_button("复制路径")
                        .on_hover_text("复制父进程可执行文件完整路径")
                        .clicked()
                    {
                        actions.push(AppAction::CopyText(path.clone()));
                    }
                    if ui
                        .small_button("打开所在位置")
                        .on_hover_text("在资源管理器中选中父进程可执行文件")
                        .clicked()
                    {
                        actions.push(AppAction::OpenFileLocation(PathBuf::from(path)));
                    }
                });
            } else {
                ui.label(RichText::new("路径不可用").color(ui.visuals().weak_text_color()));
            }
        });
    });
}

fn process_report(process: &ProcessInfo) -> String {
    let mut report = format!(
        "Windows Toolbox 进程报告\n进程：{}\nPID：{}\n父进程 PID：{}\n路径：{}\n命令行：{}\n启动时间：{}\n",
        process.name,
        process.pid,
        process
            .parent_pid
            .map_or_else(|| "无".to_owned(), |pid| pid.to_string()),
        process.exe_path.as_deref().unwrap_or("无法读取"),
        process.command_line.as_deref().unwrap_or("未读取"),
        process.started_at.as_deref().unwrap_or("无法读取"),
    );
    report.push_str("父进程链：\n");
    append_summaries(&mut report, &process.parent_chain);
    report.push_str("直接子进程：\n");
    append_summaries(&mut report, &process.children);
    report
}

fn append_summaries(report: &mut String, processes: &[ProcessSummary]) {
    if processes.is_empty() {
        report.push_str("- 无\n");
        return;
    }
    for process in processes {
        report.push_str(&format!("- {} (PID {})", process.name, process.pid));
        if let Some(path) = &process.exe_path {
            report.push_str(&format!("\n  路径：{path}"));
        }
        report.push('\n');
    }
}

#[cfg(test)]
mod tests {
    use crate::model::{ProcessInfo, ProcessSummary};

    use super::process_report;

    #[test]
    fn report_contains_children_and_parent_paths() {
        let summary = ProcessSummary {
            pid: 7,
            name: "父进程📁.exe".into(),
            exe_path: Some(r"C:\parent.exe".into()),
        };
        let process = ProcessInfo {
            pid: 8,
            parent_pid: Some(7),
            name: "child.exe".into(),
            exe_path: Some(r"C:\child.exe".into()),
            command_line: None,
            started_at: None,
            parent_chain: vec![summary.clone()],
            children: vec![ProcessSummary {
                pid: 9,
                name: "子进程😀.exe".into(),
                exe_path: None,
            }],
        };
        let report = process_report(&process);
        assert!(report.contains(r"C:\parent.exe"));
        assert!(report.contains("子进程😀.exe (PID 9)"));
        assert!(report.contains("父进程📁.exe (PID 7)"));
    }
}
