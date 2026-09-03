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
        invocation::ToolPayload,
        worker::TaskResult,
    },
    model::{
        AppError, ProcessCommandLine, ProcessInfo, ProcessRunState, ProcessSummary,
        ProcessTreeNode, ProcessTreeSnapshot,
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
    review_seeded: bool,
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
            category: ToolCategory::Diagnostic,
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

    fn ui(&mut self, ui: &mut egui::Ui, context: ToolUiContext) -> Vec<AppAction> {
        if context.review_mode && !self.review_seeded {
            self.seed_review();
        }
        let mut actions = Vec::new();
        let palette = ui::palette_for_ui(ui);
        if !context.review_mode && !self.tree_requested {
            self.tree_requested = true;
            self.tree_refresh_pending = true;
            actions.push(AppAction::LoadProcessTree);
        }
        heading(ui, "进程关系", "浏览进程树、父进程链与启动信息");
        ui.add_space(0.0);
        ui::card(ui, |ui| {
            ui.horizontal(|ui| {
                let action_width = 92.0;
                let filter_width =
                    (ui.available_width() - action_width * 2.0 - ui::SPACE_8 * 2.0).max(180.0);
                ui.add_sized(
                    [filter_width, ui::CONTROL_HEIGHT],
                    ui::text_input(&mut self.tree_filter, "搜索进程名称或 PID"),
                );
                ui::primary_button_sized(ui, "搜索", [action_width, ui::CONTROL_HEIGHT]);
                if ui::secondary_button_sized(ui, "刷新", [action_width, ui::CONTROL_HEIGHT])
                    .clicked()
                    && !context.review_mode
                {
                    self.tree_requested = true;
                    self.tree_refresh_pending = true;
                    actions.push(AppAction::LoadProcessTree);
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

                let panel_height = (ui.ctx().screen_rect().height() * 0.70).clamp(520.0, 950.0);
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
                            egui::ScrollArea::both()
                                .id_salt("process-tree-left-scroll")
                                .max_height(panel_height)
                                .min_scrolled_height(panel_height)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    render_tree_header(ui);
                                    ui.separator();
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
                                    ui.with_layout(
                                        egui::Layout::top_down(egui::Align::Min),
                                        |ui| self.render_selected_detail(ui, &mut actions),
                                    );
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
                        egui::ScrollArea::both().show(ui, |ui| {
                            render_tree_header(ui);
                            ui.separator();
                            for pid in &roots {
                                self.render_tree_node(ui, &nodes, *pid, 0, &mut actions);
                            }
                        });
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
    fn seed_review(&mut self) {
        self.review_seeded = true;
        self.tree_requested = true;
        self.tree_refresh_pending = false;
        self.selected_pid = Some(7_832);
        self.expanded.extend([4, 644, 732, 920, 5_420, 7_832]);
        let nodes = vec![
            ProcessTreeNode {
                pid: 4,
                parent_pid: None,
                name: "System".into(),
                children: vec![428, 556, 644],
                cpu_percent: Some(0.12),
                memory_bytes: Some(30_000_000),
                ..Default::default()
            },
            ProcessTreeNode {
                pid: 428,
                parent_pid: Some(4),
                name: "smss.exe".into(),
                children: Vec::new(),
                cpu_percent: Some(0.00),
                memory_bytes: Some(1_150_000),
                ..Default::default()
            },
            ProcessTreeNode {
                pid: 556,
                parent_pid: Some(4),
                name: "csrss.exe".into(),
                children: Vec::new(),
                cpu_percent: Some(0.01),
                memory_bytes: Some(2_300_000),
                ..Default::default()
            },
            ProcessTreeNode {
                pid: 644,
                parent_pid: Some(4),
                name: "wininit.exe".into(),
                children: vec![732],
                cpu_percent: Some(0.00),
                memory_bytes: Some(1_700_000),
                ..Default::default()
            },
            ProcessTreeNode {
                pid: 732,
                parent_pid: Some(644),
                name: "services.exe".into(),
                children: vec![920, 1_012],
                cpu_percent: Some(0.18),
                memory_bytes: Some(8_800_000),
                ..Default::default()
            },
            ProcessTreeNode {
                pid: 920,
                parent_pid: Some(732),
                name: "svchost.exe".into(),
                children: vec![3_152],
                cpu_percent: Some(0.05),
                memory_bytes: Some(13_100_000),
                ..Default::default()
            },
            ProcessTreeNode {
                pid: 3_152,
                parent_pid: Some(920),
                name: "WmiPrvSE.exe".into(),
                children: Vec::new(),
                cpu_percent: Some(0.01),
                memory_bytes: Some(6_600_000),
                ..Default::default()
            },
            ProcessTreeNode {
                pid: 1_012,
                parent_pid: Some(732),
                name: "svchost.exe".into(),
                children: vec![4_120],
                cpu_percent: Some(0.03),
                memory_bytes: Some(10_300_000),
                ..Default::default()
            },
            ProcessTreeNode {
                pid: 4_120,
                parent_pid: Some(1_012),
                name: "Spooler.exe".into(),
                children: Vec::new(),
                cpu_percent: Some(0.00),
                memory_bytes: Some(4_300_000),
                ..Default::default()
            },
            ProcessTreeNode {
                pid: 5_420,
                parent_pid: None,
                name: "explorer.exe".into(),
                children: vec![7_832, 6_680, 8_120],
                cpu_percent: Some(0.32),
                memory_bytes: Some(54_800_000),
                ..Default::default()
            },
            ProcessTreeNode {
                pid: 7_832,
                parent_pid: Some(5_420),
                name: "ToolDeck.exe".into(),
                children: vec![9_576],
                cpu_percent: Some(1.24),
                memory_bytes: Some(82_700_000),
                ..Default::default()
            },
            ProcessTreeNode {
                pid: 9_576,
                parent_pid: Some(7_832),
                name: "ToolDeck.Helper.exe".into(),
                children: Vec::new(),
                cpu_percent: Some(0.21),
                memory_bytes: Some(23_800_000),
                ..Default::default()
            },
            ProcessTreeNode {
                pid: 6_680,
                parent_pid: Some(5_420),
                name: "notepad.exe".into(),
                children: Vec::new(),
                cpu_percent: Some(0.00),
                memory_bytes: Some(3_000_000),
                ..Default::default()
            },
            ProcessTreeNode {
                pid: 8_120,
                parent_pid: Some(5_420),
                name: "cmd.exe".into(),
                children: Vec::new(),
                cpu_percent: Some(0.00),
                memory_bytes: Some(3_600_000),
                ..Default::default()
            },
        ];
        self.tree = Some(Ok(ProcessTreeSnapshot {
            nodes,
            roots: vec![4, 5_420],
        }));
        let executable = r"C:\Program Files\ToolDeck\ToolDeck.exe".to_owned();
        self.result = Some(Ok(ProcessInfo {
            pid: 7_832,
            parent_pid: Some(5_420),
            name: "ToolDeck.exe".into(),
            exe_path: Some(executable.clone()),
            command_line: Some(format!("\"{executable}\"")),
            started_at: Some("2026-08-31 14:21:33".into()),
            owner: Some("DESKTOP\\chen".into()),
            parent_chain: vec![
                ProcessSummary {
                    pid: 4,
                    name: "System".into(),
                    ..Default::default()
                },
                ProcessSummary {
                    pid: 5_420,
                    name: "explorer.exe".into(),
                    exe_path: Some(r"C:\Windows\explorer.exe".into()),
                    ..Default::default()
                },
                ProcessSummary {
                    pid: 7_832,
                    name: "ToolDeck.exe".into(),
                    exe_path: Some(executable),
                    ..Default::default()
                },
            ],
            children: vec![ProcessSummary {
                pid: 9_576,
                name: "ToolDeck.Helper.exe".into(),
                exe_path: Some(r"C:\Program Files\ToolDeck\ToolDeck.Helper.exe".into()),
                ..Default::default()
            }],
            ..Default::default()
        }));
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
        let widths = tree_column_widths();
        let row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let selected = self.selected_pid == Some(pid);
            let response = ui
                .allocate_ui_with_layout(
                egui::vec2(widths[0], 38.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.add_space((depth.min(12) as f32) * 16.0);
                    if node.children.is_empty() {
                        ui.add_sized([20.0, 24.0], egui::Label::new(" "));
                    } else if ui::disclosure_button(ui, expanded).clicked() {
                        if expanded {
                            self.expanded.remove(&pid);
                        } else {
                            self.expanded.insert(pid);
                        }
                    }
                    ui.add_sized(
                        [
                            (widths[0] - ((depth.min(12) as f32) * 16.0) - 20.0).max(64.0),
                            38.0,
                        ],
                        egui::Button::selectable(selected, RichText::new(&node.name).strong()),
                    )
                },
            )
                .inner;
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
                self.result = None;
                actions.push(AppAction::InspectProcess { pid });
            }
            ui.add_sized(
                [widths[1], 38.0],
                egui::Label::new(RichText::new(node.pid.to_string()).monospace()),
            );
            ui.add_sized(
                [widths[2], 38.0],
                egui::Label::new(RichText::new(format_cpu(node.cpu_percent)).monospace()),
            );
            ui.add_sized(
                [widths[3], 38.0],
                egui::Label::new(RichText::new(format_memory(node.memory_bytes)).monospace()),
            );
            ui.add_sized(
                [widths[4], 38.0],
                egui::Label::new(
                    RichText::new(node.run_state.label())
                        .color(run_state_color(node.run_state, ui::palette_for_ui(ui))),
                ),
            );
        });
        paint_tree_grid(ui, row.response.rect, widths);
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
        ui.label(RichText::new("进程详情").strong().size(17.0));
        ui.add_space(ui::SPACE_12);
        let parent_pid = process
            .parent_pid
            .map_or_else(|| "无".to_owned(), |pid| pid.to_string());
        let path = process
            .exe_path
            .as_deref()
            .unwrap_or("无法读取（受保护的系统进程可能需要管理员权限）");
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            process_detail_field(ui, "名称：", &process.name, palette.text);
            process_detail_field(ui, "PID：", &process.pid.to_string(), palette.text);
            process_detail_field(ui, "父进程 PID：", &parent_pid, palette.text);
            process_detail_field(ui, "路径：", path, palette.text);
            process_detail_field(
                ui,
                "启动时间：",
                process.started_at.as_deref().unwrap_or("无法读取"),
                palette.text,
            );
            process_detail_field(
                ui,
                "用户：",
                process.owner.as_deref().unwrap_or("无法读取"),
                palette.text,
            );
        });

        ui.add_space(ui::SPACE_8);
        ui.separator();
        ui.add_space(ui::SPACE_8);
        ui.label(RichText::new("父进程链").strong().size(15.0));
        ui.add_space(ui::SPACE_8);
        if process.parent_chain.is_empty() {
            ui.label(
                RichText::new("父进程已退出或关系不可用。")
                    .color(ui.visuals().weak_text_color()),
            );
        } else {
            ui.horizontal_wrapped(|ui| {
                for (index, ancestor) in process.parent_chain.iter().enumerate() {
                    let response = ui.add(
                        egui::Label::new(
                            RichText::new(format!("{} ({})", ancestor.name, ancestor.pid))
                                .color(palette.text),
                        )
                        .sense(egui::Sense::click()),
                    );
                    if response.clicked() {
                        actions.push(AppAction::InspectProcess { pid: ancestor.pid });
                    }
                    if index + 1 < process.parent_chain.len() {
                        ui.label(RichText::new("›").size(17.0).color(palette.weak));
                    }
                }
            });
        }

        ui.add_space(ui::SPACE_8);
        ui.separator();
        ui.add_space(ui::SPACE_8);
        ui.label(RichText::new("子进程").strong().size(15.0));
        ui.add_space(ui::SPACE_8);
        ui::table_card(ui, |ui| {
            let available_width = ui.available_width();
            let name_width = (available_width * 0.48).max(160.0);
            let pid_width = (available_width * 0.24).max(84.0);
            let state_width = (available_width - name_width - pid_width).max(80.0);
            ui.horizontal(|ui| {
                for (label, width) in [
                    ("名称", name_width),
                    ("PID", pid_width),
                    ("状态", state_width),
                ] {
                    ui.add_sized(
                        [width, 28.0],
                        egui::Label::new(RichText::new(label).strong().color(palette.weak)),
                    );
                }
            });
            ui.separator();
            if process.children.is_empty() {
                ui.add_sized(
                    [available_width, 30.0],
                    egui::Label::new(
                        RichText::new("当前快照中没有检测到存活的直接子进程。")
                            .color(palette.weak),
                    ),
                );
            }
            for child in &process.children {
                ui.horizontal(|ui| {
                    let response = ui.add_sized(
                        [name_width, 30.0],
                        egui::Label::new(RichText::new(&child.name).color(palette.text))
                            .sense(egui::Sense::click()),
                    );
                    if response.clicked() {
                        actions.push(AppAction::InspectProcess { pid: child.pid });
                    }
                    ui.add_sized(
                        [pid_width, 30.0],
                        egui::Label::new(RichText::new(child.pid.to_string()).monospace()),
                    );
                    ui.add_sized(
                        [state_width, 30.0],
                        egui::Label::new(RichText::new("运行中").color(palette.success_text)),
                    );
                });
            }
        });

        ui.add_space(ui::SPACE_8);
        ui.label(RichText::new("命令行").strong().size(15.0));
        ui.add_space(ui::SPACE_8);
        let (command_label, command_hint) = match self.command_line_state.get(&process.pid) {
            Some(CommandLineState::Loading) => ("正在读取命令行", "正在通过 WMI 读取命令行".to_owned()),
            Some(CommandLineState::Ready(Some(value))) => ("重新读取命令行", value.clone()),
            Some(CommandLineState::Ready(None)) => {
                ("重新读取命令行", "命令行为空或不可访问".to_owned())
            }
            Some(CommandLineState::Failed(error)) => ("重试读取命令行", error.clone()),
            None => ("读取命令行", "读取该进程的完整命令行".to_owned()),
        };
        if ui::secondary_button_sized(ui, command_label, [132.0, ui::CONTROL_HEIGHT])
            .on_hover_text(command_hint)
            .clicked()
        {
            actions.push(AppAction::InspectProcessCommandLine { pid: process.pid });
        }
        ui.add_space(ui::SPACE_8);
        ui.separator();
        ui.add_space(ui::SPACE_8);
        ui.horizontal(|ui| {
            if ui::secondary_button(ui, "复制路径").clicked() {
                actions.push(AppAction::CopyText(path.to_owned()));
            }
            if ui::secondary_button(ui, "打开位置").clicked() && process.exe_path.is_some() {
                actions.push(AppAction::OpenFileLocation(PathBuf::from(path)));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui::danger_button(ui, "结束进程").clicked() {
                    actions.push(AppAction::RequestTerminateProcess(process.summary()));
                }
            });
        });
        ui.add_space(ui::SPACE_8);
        ui.label(
            RichText::new("提示：结束进程将终止该进程及其所有子进程。")
                .size(12.0)
                .color(palette.weak),
        );
    }
}

/// 绘制进程详情中的固定标签和值，保持各项数据在同一阅读轨道中。
fn process_detail_field(ui: &mut egui::Ui, label: &str, value: &str, value_color: egui::Color32) {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 30.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
        ui.add_sized(
            [116.0, 20.0],
            egui::Label::new(RichText::new(label).color(ui.visuals().weak_text_color())),
        );
        ui.add(egui::Label::new(RichText::new(value).color(value_color)).wrap())
            .on_hover_text(value);
        },
    );
}

/// 进程树与列标题共用固定列宽，便于在窄面板内通过横向滚动保持字段可读。
const PROCESS_TREE_COLUMN_WIDTHS: [f32; 5] = [250.0, 70.0, 68.0, 100.0, 64.0];

fn tree_column_widths() -> [f32; 5] {
    PROCESS_TREE_COLUMN_WIDTHS
}

fn render_tree_header(ui: &mut egui::Ui) {
    let widths = tree_column_widths();
    ui.set_min_width(widths.iter().sum());
    let header = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (label, width) in [
            ("进程", widths[0]),
            ("PID", widths[1]),
            ("CPU", widths[2]),
            ("内存", widths[3]),
            ("状态", widths[4]),
        ] {
            ui.add_sized(
                [width, 30.0],
                egui::Label::new(RichText::new(label).strong().color(ui.visuals().weak_text_color())),
            );
        }
    });
    paint_tree_grid(ui, header.response.rect, widths);
}

fn paint_tree_grid(ui: &mut egui::Ui, row_rect: egui::Rect, widths: [f32; 5]) {
    let stroke = egui::Stroke::new(1.0_f32, ui::palette_for_ui(ui).border);
    let right = row_rect.left() + widths.iter().sum::<f32>();
    ui.painter().line_segment(
        [
            egui::pos2(row_rect.left(), row_rect.bottom()),
            egui::pos2(right, row_rect.bottom()),
        ],
        stroke,
    );
    let mut column_x = row_rect.left();
    for width in widths.into_iter().take(4) {
        column_x += width;
        ui.painter().line_segment(
            [
                egui::pos2(column_x, row_rect.top()),
                egui::pos2(column_x, row_rect.bottom()),
            ],
            stroke,
        );
    }
}

fn format_cpu(value: Option<f32>) -> String {
    value.map_or_else(|| "—".into(), |percent| format!("{percent:.2}%"))
}

fn format_memory(value: Option<u64>) -> String {
    const KIB: u64 = 1_024;
    const MIB: u64 = KIB * 1_024;
    const GIB: u64 = MIB * 1_024;
    match value {
        Some(bytes) if bytes >= GIB => format!("{:.1} GB", bytes as f64 / GIB as f64),
        Some(bytes) if bytes >= MIB => format!("{:.1} MB", bytes as f64 / MIB as f64),
        Some(bytes) if bytes >= KIB => format!("{:.1} KB", bytes as f64 / KIB as f64),
        Some(bytes) => format!("{bytes} B"),
        None => "—".into(),
    }
}

fn run_state_color(state: ProcessRunState, palette: ui::ThemePalette) -> egui::Color32 {
    match state {
        ProcessRunState::Running => palette.success_text,
        ProcessRunState::Suspended => palette.warning_text,
        ProcessRunState::Unknown => palette.weak,
    }
}

#[cfg(test)]
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

#[cfg(test)]
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
            ..Default::default()
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
                ..Default::default()
            }],
            ..Default::default()
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
