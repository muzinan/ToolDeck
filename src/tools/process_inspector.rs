//! 进程关系工具模块。
//! 默认加载进程树并按需读取详情；命令行通过公开 WMI 接口在后台懒加载。

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use eframe::egui::{self, RichText};

use crate::{
    core::{
        actions::AppAction,
        invocation::{ToolInvocation, ToolPayload},
        worker::TaskResult,
    },
    model::{
        AppError, ProcessCommandLine, ProcessInfo, ProcessSummary, ProcessTreeNode,
        ProcessTreeSnapshot,
    },
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
    tree_filter: String,
    tree: Option<Result<ProcessTreeSnapshot, AppError>>,
    tree_requested: bool,
    tree_refresh_pending: bool,
    expanded: HashSet<u32>,
    selected_pid: Option<u32>,
    result: Option<Result<ProcessInfo, AppError>>,
    command_line_state: HashMap<u32, CommandLineState>,
    pending_command_line: Option<u32>,
    busy: bool,
}

#[derive(Clone, Debug)]
enum CommandLineState {
    Loading,
    Ready(Option<String>),
    Failed(String),
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
        let palette = ui::palette_for_ui(ui);
        if !self.tree_requested {
            self.tree_requested = true;
            self.tree_refresh_pending = true;
            actions.push(AppAction::LoadProcessTree);
        }
        heading(
            ui,
            "进程关系与拓扑分析",
            "基于 Windows Toolhelp 快照解析完整进程树、父子进程链与启动命令行参数。",
        );
        ui.add_space(ui::SPACE_16);
        ui::tech_card(ui, palette.accent, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.add_sized(
                    [ui.available_width().max(120.0), ui::CONTROL_HEIGHT],
                    ui::text_input(&mut self.tree_filter, "🔍 筛选进程名称或 PID"),
                );
                if ui::secondary_button(ui, "刷新全部进程树").clicked() {
                    self.tree_requested = true;
                    self.tree_refresh_pending = true;
                    actions.push(AppAction::LoadProcessTree);
                }
            });
            ui.add_space(ui::SPACE_8);
            ui.horizontal_wrapped(|ui| {
                let input = ui.add_sized(
                    [ui.available_width().clamp(160.0, 360.0), ui::CONTROL_HEIGHT],
                    ui::text_input(&mut self.pid_input, "输入指定 PID 精确分析"),
                );
                let submit =
                    input.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                if ui::primary_button(ui, "查看指定进程").clicked() || submit {
                    actions.extend(self.start_query());
                }
            });
        });

        if self.busy {
            ui.add_space(ui::SPACE_16);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("正在通过 Toolhelp 快照与 WMI 读取进程拓扑...").size(13.5));
            });
        }

        ui.add_space(ui::SPACE_16);
        match &self.tree {
            Some(Ok(snapshot)) => {
                let nodes = snapshot
                    .nodes
                    .iter()
                    .cloned()
                    .map(|node| (node.pid, node))
                    .collect::<HashMap<_, _>>();
                let roots = snapshot.roots.clone();

                // 顶部进程指标磁贴
                let selected_str = self
                    .selected_pid
                    .map_or_else(|| "未选择".to_owned(), |pid| format!("PID {pid}"));
                let tile_w = ((ui.available_width() - ui::SPACE_12 * 2.0) / 3.0).max(140.0);
                ui.horizontal_wrapped(|ui| {
                    ui::metric_tile(
                        ui,
                        tile_w,
                        "存活进程总数",
                        &snapshot.nodes.len().to_string(),
                        "个节点",
                        palette.accent,
                    );
                    ui::metric_tile(
                        ui,
                        tile_w,
                        "根进程分支数",
                        &roots.len().to_string(),
                        "个根系",
                        palette.accent_secondary,
                    );
                    ui::metric_tile(
                        ui,
                        tile_w,
                        "当前选中目标",
                        &selected_str,
                        "",
                        palette.warning_text,
                    );
                });
                ui.add_space(ui::SPACE_16);

                let panel_height = (ui.ctx().screen_rect().height() * 0.58).clamp(420.0, 950.0);
                let detail_width = ui.available_width();
                if detail_width >= 760.0 {
                    ui.columns(2, |columns| {
                        columns[0].set_min_width(300.0);
                        ui::card(&mut columns[0], |ui| {
                            ui.label(
                                RichText::new("全部存活进程树")
                                    .strong()
                                    .size(15.0)
                                    .color(palette.text),
                            );
                            ui.add_space(ui::SPACE_8);
                            egui::ScrollArea::vertical()
                                .id_salt("process-tree-left-scroll")
                                .max_height(panel_height)
                                .min_scrolled_height(panel_height)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_min_height(panel_height);
                                    for pid in &roots {
                                        self.render_tree_node(ui, &nodes, *pid, 0, &mut actions);
                                    }
                                });
                        });
                        ui::card(&mut columns[1], |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("process-detail-right-scroll")
                                .max_height(panel_height)
                                .min_scrolled_height(panel_height)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    ui.set_min_height(panel_height);
                                    self.render_selected_detail(ui, &mut actions);
                                });
                        });
                    });
                } else {
                    ui::card(ui, |ui| {
                        ui.label(
                            RichText::new("全部存活进程树")
                                .strong()
                                .size(15.0)
                                .color(palette.text),
                        );
                        ui.add_space(ui::SPACE_8);
                        for pid in &roots {
                            self.render_tree_node(ui, &nodes, *pid, 0, &mut actions);
                        }
                    });
                    ui.add_space(ui::SPACE_16);
                    ui::card(ui, |ui| self.render_selected_detail(ui, &mut actions));
                }
            }
            Some(Err(error)) => error_state(ui, error),
            None if !self.busy => empty_state(
                ui,
                "正在准备全部进程树",
                "首次打开会在后台读取当前存活进程快照；界面保持流畅响应。",
            ),
            None => {}
        }
        actions
    }

    fn handle_invocation(&mut self, payload: ToolPayload) -> Vec<AppAction> {
        match payload {
            ToolPayload::Process { pid } => {
                self.pid_input = pid.to_string();
                self.selected_pid = Some(pid);
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
                if let Ok(process) = &result {
                    self.selected_pid = Some(process.pid);
                    self.command_line_state
                        .insert(process.pid, CommandLineState::Loading);
                    self.pending_command_line = Some(process.pid);
                }
                self.result = Some(result);
            }
            TaskResult::ProcessTree(result) => {
                self.busy = false;
                self.tree_refresh_pending = false;
                self.tree = Some(result);
            }
            TaskResult::ProcessCommandLine { pid, result } => {
                self.busy = false;
                match result {
                    Ok(ProcessCommandLine {
                        pid: result_pid,
                        command_line,
                    }) => {
                        if let Some(Ok(process)) = &mut self.result
                            && process.pid == result_pid
                        {
                            process.command_line = command_line.clone();
                        }
                        self.command_line_state
                            .insert(result_pid, CommandLineState::Ready(command_line));
                    }
                    Err(error) => {
                        self.command_line_state.insert(
                            pid,
                            CommandLineState::Failed(command_line_failure_message(pid, &error)),
                        );
                    }
                }
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

    fn poll_actions(&mut self, _now: std::time::Instant) -> Vec<AppAction> {
        self.pending_command_line
            .take()
            .map(|pid| vec![AppAction::InspectProcessCommandLine { pid }])
            .unwrap_or_default()
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

    fn render_selected_detail(&self, ui: &mut egui::Ui, actions: &mut Vec<AppAction>) {
        match (self.selected_pid, &self.result) {
            (Some(pid), Some(Ok(process))) if process.pid == pid => {
                self.render_process(ui, process, actions);
            }
            (_, Some(Err(error))) => error_state(ui, error),
            (Some(pid), _) => empty_state(
                ui,
                "正在读取选中进程",
                &format!("PID {pid} 的详情将在后台加载。"),
            ),
            (None, _) => empty_state(
                ui,
                "选择一个进程查看详情",
                "点击左侧树节点后，这里会显示路径、命令行、启动时间和父子关系。",
            ),
        }
    }

    fn render_tree_node(
        &mut self,
        ui: &mut egui::Ui,
        nodes: &HashMap<u32, ProcessTreeNode>,
        pid: u32,
        depth: usize,
        actions: &mut Vec<AppAction>,
    ) {
        let Some(node) = nodes.get(&pid) else {
            return;
        };
        if !self.node_matches(nodes, pid) {
            return;
        }
        let expanded = self.expanded.contains(&pid)
            || (!self.tree_filter.trim().is_empty()
                && self.node_has_matching_descendant(nodes, pid));
        ui.horizontal(|ui| {
            ui.add_space((depth.min(16) as f32) * 16.0);
            if node.children.is_empty() {
                ui.add_sized([20.0, 24.0], egui::Label::new(" "));
            } else if ui
                .add_sized(
                    [20.0, 24.0],
                    egui::Button::new(if expanded { "v" } else { ">" }),
                )
                .clicked()
            {
                if expanded {
                    self.expanded.remove(&pid);
                } else {
                    self.expanded.insert(pid);
                }
            }
            let selected = self.selected_pid == Some(pid);
            let response = ui.add(
                egui::Label::new(
                    RichText::new(format!("{}  (PID {})", node.name, node.pid))
                        .strong()
                        .color(if selected {
                            ui::accent(ui)
                        } else {
                            ui.visuals().text_color()
                        }),
                )
                .wrap()
                .sense(egui::Sense::click()),
            );
            response
                .clone()
                .on_hover_text(format!("{} (PID {})", node.name, node.pid));
            if selected || response.has_focus() {
                ui.painter().rect_stroke(
                    response.rect.expand(2.0),
                    egui::CornerRadius::same(4),
                    egui::Stroke::new(1.0_f32, ui::accent(ui)),
                    egui::StrokeKind::Middle,
                );
            }
            if response.clicked() {
                response.request_focus();
            }
            if response.clicked()
                || (response.has_focus()
                    && ui.input(|input| {
                        input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
                    }))
            {
                self.selected_pid = Some(pid);
                self.pid_input = pid.to_string();
                self.result = None;
                actions.push(AppAction::InspectProcess { pid });
            }
        });
        if expanded {
            for child_pid in &node.children {
                self.render_tree_node(ui, nodes, *child_pid, depth + 1, actions);
            }
        }
    }

    fn node_matches(&self, nodes: &HashMap<u32, ProcessTreeNode>, pid: u32) -> bool {
        let filter = self.tree_filter.trim().to_ascii_lowercase();
        if filter.is_empty() {
            return true;
        }
        let Some(node) = nodes.get(&pid) else {
            return false;
        };
        node.name.to_ascii_lowercase().contains(&filter)
            || node.pid.to_string().contains(&filter)
            || self.node_has_matching_descendant(nodes, pid)
    }

    fn node_has_matching_descendant(
        &self,
        nodes: &HashMap<u32, ProcessTreeNode>,
        pid: u32,
    ) -> bool {
        let filter = self.tree_filter.trim().to_ascii_lowercase();
        nodes.get(&pid).is_some_and(|node| {
            node.children.iter().any(|child_pid| {
                nodes.get(child_pid).is_some_and(|child| {
                    child.name.to_ascii_lowercase().contains(&filter)
                        || child.pid.to_string().contains(&filter)
                        || self.node_has_matching_descendant(nodes, *child_pid)
                })
            })
        })
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
            wrapped_detail_label(ui, RichText::new(path).color(ui::accent(ui))).on_hover_text(path);

            ui.add_space(ui::SPACE_12);
            ui.strong("命令行参数");
            ui.add_space(4.0);
            let command_line = match self.command_line_state.get(&process.pid) {
                Some(CommandLineState::Loading) => "正在通过 WMI 读取命令行...".to_owned(),
                Some(CommandLineState::Ready(Some(value))) => value.clone(),
                Some(CommandLineState::Ready(None)) => {
                    "WMI 未返回命令行（可能为空或权限不足）".into()
                }
                Some(CommandLineState::Failed(error)) => format!("命令行不可用：{error}"),
                None => "等待后台读取命令行...".into(),
            };
            wrapped_detail_label(
                ui,
                RichText::new(&command_line).color(ui.visuals().weak_text_color()),
            )
            .on_hover_text(&command_line);

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
                            if ui::small_action_button(ui, "查看父进程").clicked() {
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
                wrapped_detail_label(
                    ui,
                    RichText::new(path)
                        .small()
                        .color(ui.visuals().weak_text_color()),
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

fn wrapped_detail_label(ui: &mut egui::Ui, text: RichText) -> egui::Response {
    ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
        ui.add(egui::Label::new(text).wrap())
    })
    .inner
}

fn command_line_failure_message(pid: u32, error: &AppError) -> String {
    match error {
        AppError::ProcessExited(_) => format!("PID {pid} 已退出"),
        _ => {
            let (title, detail) = error.user_message();
            format!("{title}：{detail}")
        }
    }
}

#[cfg(test)]
mod tests {
    use eframe::egui;

    use crate::{
        core::worker::TaskResult,
        model::{AppError, ProcessInfo, ProcessSummary},
        tools::ToolModule,
    };

    use super::{CommandLineState, ProcessInspectorTool, process_report};

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

    #[test]
    fn command_line_failure_stays_with_requested_pid() {
        let mut tool = ProcessInspectorTool {
            selected_pid: Some(22),
            ..Default::default()
        };

        tool.handle_task_result(TaskResult::ProcessCommandLine {
            pid: 11,
            result: Err(AppError::ProcessExited(11)),
        });

        assert!(matches!(
            tool.command_line_state.get(&11),
            Some(CommandLineState::Failed(message)) if message == "PID 11 已退出"
        ));
        assert!(!tool.command_line_state.contains_key(&22));
    }

    #[test]
    fn detail_text_layout_disables_column_justification() {
        egui::__run_test_ui(|ui| {
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    let (_, galley, _) = egui::Label::new(r"D:\Program Files")
                        .wrap()
                        .layout_in_ui(ui);
                    assert!(!galley.job.justify);
                });
            });
        });
    }
}
