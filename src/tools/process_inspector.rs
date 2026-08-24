//! 进程关系工具模块。
//! 使用父进程链解释进程来源；命令行字段在 V0.1 明确标注为未读取，避免依赖未公开 NT API。

use std::path::PathBuf;

use eframe::egui::{self, RichText, TextEdit};

use crate::{
    core::{actions::AppAction, invocation::ToolPayload, worker::TaskResult},
    model::{AppError, ProcessInfo},
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
            let render_controls = |ui: &mut egui::Ui| {
                ui.add_sized(
                    [
                        if matches!(
                            ui::action_layout(ui.available_width()),
                            ui::ActionLayout::Horizontal
                        ) {
                            220.0
                        } else {
                            ui.available_width()
                        },
                        32.0,
                    ],
                    TextEdit::singleline(&mut self.pid_input).hint_text("输入 PID"),
                );
                if ui
                    .add_enabled_ui(!self.busy, |ui| ui::primary_button(ui, "查看进程"))
                    .inner
                    .clicked()
                {
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
        ui::card(ui, |ui| {
            let render_actions = |ui: &mut egui::Ui| {
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
                    let action_width = 164.0;
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
                            egui::Layout::right_to_left(egui::Align::Center),
                            render_actions,
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
            ui.add(egui::Label::new(RichText::new(path).monospace()).wrap())
                .on_hover_text(path);
            if let Some(path) = &process.exe_path
                && ui.small_button("打开所在位置").clicked()
            {
                actions.push(AppAction::OpenFileLocation(PathBuf::from(path)));
            }
            ui.add_space(ui::SPACE_12);
            ui.strong("命令行");
            let command_line = process
                .command_line
                .as_deref()
                .unwrap_or("V0.1 未使用未公开 API 读取命令行");
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
                ui.horizontal(|ui| {
                    // 限制深层链的缩进，避免异常进程链将可点击名称挤出内容区。
                    ui.add_space((index.min(8) as f32) * 16.0);
                    if index > 0 {
                        ui.monospace("└─");
                    }
                    let label = format!("{}  (PID {})", ancestor.name, ancestor.pid);
                    if ui
                        .add(
                            egui::Label::new(RichText::new(&label).color(ui::accent(ui)))
                                .wrap()
                                .sense(egui::Sense::click()),
                        )
                        .on_hover_text(&label)
                        .clicked()
                    {
                        actions.push(AppAction::InspectProcess { pid: ancestor.pid });
                    }
                });
            }
        });
    }
}
