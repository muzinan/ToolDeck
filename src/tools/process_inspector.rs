//! 进程关系工具模块。
//! 使用父进程链解释进程来源；命令行字段在 V0.1 明确标注为未读取，避免依赖未公开 NT API。

use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText, TextEdit};

use crate::{
    core::{actions::AppAction, invocation::ToolPayload, worker::TaskResult},
    model::{AppError, ProcessInfo},
    tools::{
        ToolModule, ToolUiContext,
        file_lock::{empty_state, error_state, heading},
        registry::{ToolCategory, ToolDescriptor},
    },
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
            icon: "P",
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
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.add_sized(
                [220.0, 32.0],
                TextEdit::singleline(&mut self.pid_input).hint_text("输入 PID"),
            );
            if ui
                .add_enabled(!self.busy, egui::Button::new("查看进程"))
                .clicked()
            {
                actions.extend(self.start_query());
            }
        });

        if self.busy {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("正在读取进程信息...");
            });
        }

        if let Some(result) = &self.result {
            ui.add_space(20.0);
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

    fn is_busy(&self) -> bool {
        self.busy
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
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(RichText::new(&process.name).heading().strong());
                ui.label(format!("PID {}", process.pid));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(RichText::new("结束进程").color(Color32::from_rgb(184, 62, 62)))
                    .clicked()
                {
                    actions.push(AppAction::RequestTerminateProcess(process.summary()));
                }
                if ui.small_button("复制 PID").clicked() {
                    actions.push(AppAction::CopyText(process.pid.to_string()));
                }
            });
        });
        ui.add_space(14.0);
        egui::Grid::new("process-details")
            .num_columns(2)
            .spacing([18.0, 8.0])
            .show(ui, |ui| {
                ui.strong("路径");
                let path = process
                    .exe_path
                    .as_deref()
                    .unwrap_or("无法读取，可能需要管理员权限");
                ui.horizontal(|ui| {
                    ui.label(path);
                    if let Some(path) = &process.exe_path {
                        if ui.small_button("打开所在位置").clicked() {
                            actions.push(AppAction::OpenFileLocation(PathBuf::from(path)));
                        }
                    }
                });
                ui.end_row();
                ui.strong("命令行");
                ui.label(
                    process
                        .command_line
                        .as_deref()
                        .unwrap_or("V0.1 未使用未公开 API 读取命令行"),
                );
                ui.end_row();
                ui.strong("启动时间");
                ui.label(process.started_at.as_deref().unwrap_or("无法读取"));
                ui.end_row();
                ui.strong("父进程 PID");
                ui.label(
                    process
                        .parent_pid
                        .map(|pid| pid.to_string())
                        .unwrap_or_else(|| "无".into()),
                );
                ui.end_row();
            });
        ui.add_space(18.0);
        ui.label(RichText::new("启动来源").strong());
        ui.add_space(7.0);
        if process.parent_chain.is_empty() {
            ui.label("父进程关系不可用。");
        }
        for (index, ancestor) in process.parent_chain.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.add_space(index as f32 * 20.0);
                if index > 0 {
                    ui.monospace("└─");
                }
                if ui
                    .link(format!("{}  (PID {})", ancestor.name, ancestor.pid))
                    .clicked()
                {
                    actions.push(AppAction::InspectProcess { pid: ancestor.pid });
                }
            });
        }
    }
}
