//! 进程关系工具模块。
//! 使用父进程链解释进程来源；命令行字段在当前版本明确标注为未读取，避免依赖未公开 NT API。

use std::path::PathBuf;

use eframe::egui::{self, RichText};

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
            "通过 Toolhelp 快照查看进程详情与完整父进程链，追溯进程启动来源。",
        );
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            match ui::action_layout(ui.available_width()) {
                ui::ActionLayout::Horizontal => {
                    let mut submit = false;
                    ui.horizontal(|ui| {
                        let button_width = 110.0;
                        let input_width =
                            (ui.available_width() - button_width - ui.spacing().item_spacing.x)
                                .max(160.0);
                        let input = ui.add_sized(
                            [input_width, ui::CONTROL_HEIGHT],
                            ui::text_input(&mut self.pid_input, "输入目标进程 PID (如 1234)"),
                        );
                        submit |= input.lost_focus()
                            && ui.input(|input| input.key_pressed(egui::Key::Enter));
                        if ui::primary_button_sized(
                            ui,
                            if self.busy {
                                "重新查询"
                            } else {
                                "查看进程"
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
                        ui::text_input(&mut self.pid_input, "输入目标进程 PID (如 1234)"),
                    );
                    let submit =
                        input.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    ui.add_space(ui::SPACE_8);
                    if ui::primary_button(
                        ui,
                        if self.busy {
                            "重新查询"
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
                ui.label(RichText::new("正在检索系统进程树快照与进程信息...").size(13.5));
            });
        }

        if let Some(result) = &self.result {
            ui.add_space(ui::SPACE_16);
            match result {
                Ok(process) => self.render_process(ui, process, &mut actions),
                Err(error) => error_state(ui, error),
            }
        } else if !self.busy {
            ui.add_space(36.0);
            empty_state(
                ui,
                "输入 PID 查看进程来源与层级",
                "支持直接输入 PID 查询；端口占用与文件占用页面中的进程名称也可一键点击跳转至此。",
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
        let palette = ui::palette_for_ui(ui);
        ui::card(ui, |ui| match ui::action_layout(ui.available_width()) {
            ui::ActionLayout::Horizontal => {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(RichText::new(&process.name).size(18.0).strong()).wrap(),
                    )
                    .on_hover_text(&process.name);
                    ui::badge(
                        ui,
                        &format!("PID: {}", process.pid),
                        palette.accent,
                        palette.accent.gamma_multiply(if ui.visuals().dark_mode {
                            0.22
                        } else {
                            0.12
                        }),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::danger_button(ui, "结束进程").clicked() {
                            actions.push(AppAction::RequestTerminateProcess(process.summary()));
                        }
                        if ui::secondary_button(ui, "复制报告").clicked() {
                            actions.push(AppAction::CopyText(process_report(process)));
                        }
                        if ui::secondary_button(ui, "复制 PID").clicked() {
                            actions.push(AppAction::CopyText(process.pid.to_string()));
                        }
                        if ui::secondary_button(ui, "查看端口占用").clicked() {
                            actions.push(AppAction::InvokeTool(ToolInvocation::ports_for_process(
                                process.pid,
                            )));
                        }
                        if ui::primary_button(ui, "刷新").clicked() {
                            actions.push(AppAction::InspectProcess { pid: process.pid });
                        }
                    });
                });
            }
            ui::ActionLayout::Vertical => {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(RichText::new(&process.name).size(18.0).strong()).wrap(),
                    )
                    .on_hover_text(&process.name);
                    ui::badge(
                        ui,
                        &format!("PID: {}", process.pid),
                        palette.accent,
                        palette.accent.gamma_multiply(if ui.visuals().dark_mode {
                            0.22
                        } else {
                            0.12
                        }),
                    );
                });
                ui.add_space(ui::SPACE_8);
                ui.horizontal_wrapped(|ui| {
                    if ui::primary_button(ui, "刷新").clicked() {
                        actions.push(AppAction::InspectProcess { pid: process.pid });
                    }
                    if ui::secondary_button(ui, "查看端口占用").clicked() {
                        actions.push(AppAction::InvokeTool(ToolInvocation::ports_for_process(
                            process.pid,
                        )));
                    }
                    if ui::secondary_button(ui, "复制 PID").clicked() {
                        actions.push(AppAction::CopyText(process.pid.to_string()));
                    }
                    if ui::secondary_button(ui, "复制报告").clicked() {
                        actions.push(AppAction::CopyText(process_report(process)));
                    }
                    if ui::danger_button(ui, "结束进程").clicked() {
                        actions.push(AppAction::RequestTerminateProcess(process.summary()));
                    }
                });
            }
        });
        ui.add_space(ui::SPACE_12);
        ui::card(ui, |ui| {
            ui.label(RichText::new("基础信息与属性").strong().size(14.5));
            ui.add_space(ui::SPACE_12);

            ui.horizontal(|ui| {
                ui.strong("可执行文件路径");
                if process.exe_path.is_some() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::small_action_button(ui, "定位文件")
                            .on_hover_text("在资源管理器中选中当前进程可执行文件")
                            .clicked()
                            && let Some(path) = &process.exe_path
                        {
                            actions.push(AppAction::OpenFileLocation(PathBuf::from(path)));
                        }
                        if ui::small_action_button(ui, "复制路径")
                            .on_hover_text("复制当前进程可执行文件完整路径")
                            .clicked()
                            && let Some(path) = &process.exe_path
                        {
                            actions.push(AppAction::CopyText(path.clone()));
                        }
                    });
                }
            });
            ui.add_space(4.0);
            let path = process
                .exe_path
                .as_deref()
                .unwrap_or("无法读取（受保护的系统进程可能需要管理员权限）");
            ui.add(egui::Label::new(RichText::new(path).monospace().color(ui::accent(ui))).wrap())
                .on_hover_text(path);

            ui.add_space(ui::SPACE_12);
            ui.strong("命令行参数");
            ui.add_space(4.0);
            let command_line = process
                .command_line
                .as_deref()
                .unwrap_or("未读取（当前版本为系统兼容性不依赖未公开 NT API）");
            ui.add(
                egui::Label::new(
                    RichText::new(command_line)
                        .monospace()
                        .color(ui.visuals().weak_text_color()),
                )
                .wrap(),
            )
            .on_hover_text(command_line);

            ui.add_space(ui::SPACE_16);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.strong("启动时间");
                    ui.add_space(4.0);
                    ui.label(process.started_at.as_deref().unwrap_or("无法读取"));
                });
                ui.add_space(ui::SPACE_32);
                ui.vertical(|ui| {
                    ui.strong("父进程 PID");
                    ui.add_space(4.0);
                    if let Some(parent_pid) = process.parent_pid {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(parent_pid.to_string()).monospace().size(13.5));
                            if ui::small_action_button(ui, "查看父进程 ➔").clicked() {
                                actions.push(AppAction::InspectProcess { pid: parent_pid });
                            }
                        });
                    } else {
                        ui.label("无");
                    }
                });
            });
        });
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            ui.label(
                RichText::new("启动来源与父进程链 (Ancestor Chain)")
                    .strong()
                    .size(14.5),
            );
            ui.add_space(ui::SPACE_8);
            if process.parent_chain.is_empty() {
                ui.label(
                    RichText::new("父进程已退出或关系不可用。")
                        .color(ui.visuals().weak_text_color()),
                );
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
            ui.label(
                RichText::new("直接子进程 (Child Processes)")
                    .strong()
                    .size(14.5),
            );
            ui.add_space(ui::SPACE_8);
            if process.children.is_empty() {
                ui.label(
                    RichText::new("当前快照中没有检测到存活的直接子进程。")
                        .color(ui.visuals().weak_text_color()),
                );
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
    let palette = ui::palette_for_ui(ui);
    ui.horizontal(|ui| {
        // 深层链最多缩进八级
        ui.add_space((depth.min(8) as f32) * 16.0);
        if depth > 0 {
            ui.colored_label(palette.weak, "↳");
        }
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let response = ui.add(
                    egui::Label::new(RichText::new(&process.name).strong().color(ui::accent(ui)))
                        .wrap()
                        .sense(egui::Sense::click()),
                );
                response.clone().on_hover_text(format!(
                    "点击查看进程详情：{} (PID {})",
                    process.name, process.pid
                ));
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
                            input.key_pressed(egui::Key::Enter)
                                || input.key_pressed(egui::Key::Space)
                        }))
                {
                    actions.push(AppAction::InspectProcess { pid: process.pid });
                }
                ui::badge(
                    ui,
                    &format!("PID: {}", process.pid),
                    palette.weak,
                    palette.border_subtle,
                );
            });
            if let Some(path) = &process.exe_path {
                ui.add_space(1.0);
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
                ui.add_space(2.0);
                ui.horizontal_wrapped(|ui| {
                    if ui::small_action_button(ui, "复制路径")
                        .on_hover_text("复制父进程可执行文件完整路径")
                        .clicked()
                    {
                        actions.push(AppAction::CopyText(path.clone()));
                    }
                    if ui::small_action_button(ui, "定位文件")
                        .on_hover_text("在资源管理器中选中父进程可执行文件")
                        .clicked()
                    {
                        actions.push(AppAction::OpenFileLocation(PathBuf::from(path)));
                    }
                });
            } else {
                ui.label(
                    RichText::new("路径不可用")
                        .size(12.0)
                        .color(ui.visuals().weak_text_color()),
                );
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
