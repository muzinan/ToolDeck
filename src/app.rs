//! Windows Toolbox 的 GUI 外壳。
//! 该模块拥有导航、主题、设置、后台任务协作与 Tool Invocation 路由；工具业务保留在独立模块中。

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};

use eframe::{
    egui,
    egui::{Color32, RichText},
};

use crate::{
    core::{
        actions::AppAction,
        invocation::ToolInvocation,
        worker::{RequestId, TaskDispatcher, TaskEvent, TaskEventEnvelope, TaskRequest},
    },
    model::{AppError, ProcessSummary},
    platform::windows::{
        open_directory, open_file_location, pick_file, save_csv, shell_context_menu,
    },
    settings::{AppSettings, SettingsStore, ThemePreference},
    tools::{ToolDescriptor, ToolRegistry, ToolUiContext, build_registry},
    ui,
};

#[derive(Clone, Debug, PartialEq, Eq)]
enum Page {
    Home,
    Tool(String),
    Settings,
    About,
}

/// 轻量级成功或失败提示；错误细节会留在对应工具页，外壳提示只说明刚发生的操作结果。
#[derive(Clone, Debug)]
struct Notice {
    message: String,
    tone: NoticeTone,
    expires_at: Instant,
}

/// 通知的语义类别在绘制时映射到当前主题颜色，主题切换后不保留旧色值。
#[derive(Clone, Copy, Debug)]
enum NoticeTone {
    Success,
    Danger,
}

/// Tool Host：统一协调 Tool Registry、UI、后台任务和系统集成。
pub struct ToolboxApp {
    registry: ToolRegistry,
    page: Page,
    search: String,
    settings_store: SettingsStore,
    settings: AppSettings,
    task_dispatcher: TaskDispatcher,
    task_receiver: Receiver<TaskEventEnvelope>,
    latest_requests: HashMap<&'static str, RequestId>,
    invocation_receiver: Receiver<ToolInvocation>,
    pending_invocations: Vec<ToolInvocation>,
    pending_termination: Option<ProcessSummary>,
    notice: Option<Notice>,
}

impl ToolboxApp {
    pub fn new(
        creation_context: &eframe::CreationContext<'_>,
        settings_store: SettingsStore,
        settings: AppSettings,
        settings_warning: Option<String>,
        invocation_receiver: Receiver<ToolInvocation>,
        initial_invocation: Option<ToolInvocation>,
    ) -> Result<Self, AppError> {
        let mut settings = settings;
        let registry = build_registry();
        let favorites_before = settings.favorite_tools.clone();
        normalize_favorite_tools(&registry.descriptors(), &mut settings.favorite_tools);
        if settings.favorite_tools != favorites_before {
            let _ = settings_store.save(&settings);
        }
        ui::install_windows_fonts(&creation_context.egui_ctx);
        ui::configure_styles(&creation_context.egui_ctx);
        apply_theme(&creation_context.egui_ctx, settings.theme);
        let (task_dispatcher, task_receiver) = TaskDispatcher::new()?;
        Ok(Self {
            registry,
            page: Page::Home,
            search: String::new(),
            settings_store,
            settings,
            task_dispatcher,
            task_receiver,
            latest_requests: HashMap::new(),
            invocation_receiver,
            pending_invocations: initial_invocation.into_iter().collect(),
            pending_termination: None,
            notice: settings_warning.map(|message| Notice {
                message,
                tone: NoticeTone::Danger,
                expires_at: Instant::now() + Duration::from_secs(10),
            }),
        })
    }

    fn drain_background_messages(&mut self, context: &egui::Context) {
        while let Ok(envelope) = self.task_receiver.try_recv() {
            let is_finished = matches!(&envelope.event, TaskEvent::Finished(_));
            if !accept_task_event(
                &mut self.latest_requests,
                envelope.target_tool_id,
                envelope.request_id,
                is_finished,
            ) {
                continue;
            }
            match envelope.event {
                TaskEvent::Progress {
                    message,
                    completed,
                    total,
                } => {
                    if let Some(tool) = self.registry.get_mut(envelope.target_tool_id) {
                        tool.handle_task_progress(message, completed, total);
                    }
                }
                TaskEvent::Finished(result) => {
                    if let Some(error) = result.error() {
                        log_runtime_error(error);
                    }
                    if let Some(tool) = self.registry.get_mut(envelope.target_tool_id) {
                        tool.handle_task_result(result);
                    }
                }
            }
        }

        while let Ok(invocation) = self.invocation_receiver.try_recv() {
            self.pending_invocations.push(invocation);
            // 由次实例唤起时恢复并聚焦现有窗口，平台后端会将这些命令映射到原生窗口操作。
            context.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            context.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            context.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    fn render_sidebar(&mut self, context: &egui::Context) -> Vec<AppAction> {
        let descriptors = if self.search.trim().is_empty() {
            Vec::new()
        } else {
            self.registry.search(&self.search)
        };
        let categories = self.registry.categories();
        let mut actions = Vec::new();

        egui::SidePanel::left("toolbox-sidebar")
            .resizable(false)
            .default_width(248.0)
            .min_width(248.0)
            .show(context, |ui| {
                ui.add_space(ui::SPACE_16);
                ui.horizontal(|ui| {
                    ui::tool_icon(ui, crate::tools::ToolIcon::Process, 30.0, ui::accent(ui));
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Windows Toolbox").strong().size(17.0));
                        ui.label(
                            RichText::new("系统诊断工具集")
                                .size(12.0)
                                .color(ui.visuals().weak_text_color()),
                        );
                    });
                });
                ui.add_space(ui::SPACE_16);
                ui.add_sized(
                    [ui.available_width(), 34.0],
                    egui::TextEdit::singleline(&mut self.search).hint_text("搜索工具"),
                );
                ui.add_space(ui::SPACE_16);

                if !self.search.trim().is_empty() {
                    nav_section_label(ui, "搜索结果");
                    ui.add_space(ui::SPACE_4);
                    if descriptors.is_empty() {
                        ui.small("没有匹配的工具");
                    }
                    for descriptor in descriptors {
                        if tool_nav_button(
                            ui,
                            &descriptor,
                            self.page == Page::Tool(descriptor.id.into()),
                        ) {
                            actions.push(AppAction::NavigateTo(descriptor.id.into()));
                        }
                    }
                } else {
                    if nav_text_button(ui, "常用工具", self.page == Page::Home) {
                        actions.push(AppAction::NavigateTo("home".into()));
                    }
                    let favorites: Vec<_> = self
                        .registry
                        .descriptors()
                        .into_iter()
                        .filter(|descriptor| {
                            self.settings
                                .favorite_tools
                                .iter()
                                .any(|id| id == descriptor.id)
                        })
                        .collect();
                    if !favorites.is_empty() {
                        ui.add_space(ui::SPACE_16);
                        nav_section_label(ui, "收藏");
                        ui.add_space(ui::SPACE_4);
                        for descriptor in favorites {
                            if tool_nav_button(
                                ui,
                                &descriptor,
                                self.page == Page::Tool(descriptor.id.into()),
                            ) {
                                actions.push(AppAction::NavigateTo(descriptor.id.into()));
                            }
                        }
                    }
                    ui.add_space(ui::SPACE_16);
                    for category in categories {
                        nav_section_label(ui, category.label());
                        ui.add_space(ui::SPACE_4);
                        for descriptor in self.registry.in_category(category) {
                            if tool_nav_button(
                                ui,
                                &descriptor,
                                self.page == Page::Tool(descriptor.id.into()),
                            ) {
                                actions.push(AppAction::NavigateTo(descriptor.id.into()));
                            }
                        }
                        ui.add_space(ui::SPACE_12);
                    }
                }

                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.separator();
                    ui.add_space(ui::SPACE_4);
                    if nav_text_button(ui, "关于", self.page == Page::About) {
                        actions.push(AppAction::NavigateTo("about".into()));
                    }
                    if nav_text_button(ui, "设置", self.page == Page::Settings) {
                        actions.push(AppAction::NavigateTo("settings".into()));
                    }
                });
            });
        actions
    }

    fn render_content(&mut self, context: &egui::Context) -> Vec<AppAction> {
        let page = self.page.clone();
        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(context.style().as_ref())
                    .inner_margin(egui::Margin::same(24)),
            )
            .show(context, |ui| {
                ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width().min(1040.0), ui.available_height()),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| match page {
                            Page::Home => self.render_home(ui),
                            Page::Tool(id) => self
                                .registry
                                .get_mut(&id)
                                .map(|tool| tool.ui(ui, ToolUiContext))
                                .unwrap_or_else(|| vec![AppAction::NavigateTo("home".into())]),
                            Page::Settings => self.render_settings(ui),
                            Page::About => self.render_about(ui),
                        },
                    )
                    .inner
                })
                .inner
            })
            .inner
    }

    fn render_home(&self, ui: &mut egui::Ui) -> Vec<AppAction> {
        let mut actions = Vec::new();
        ui::page_heading(ui, "常用工具", "快速进入常用的系统诊断工具。");
        ui.add_space(ui::SPACE_24);
        let descriptors = self.registry.descriptors();
        if ui.available_width() >= 728.0 {
            ui.columns(2, |columns| {
                for (index, descriptor) in descriptors.iter().enumerate() {
                    let (open, favorite) = home_tool_card(
                        &mut columns[index % 2],
                        descriptor,
                        self.settings
                            .favorite_tools
                            .iter()
                            .any(|id| id == descriptor.id),
                    );
                    if open {
                        actions.push(AppAction::NavigateTo(descriptor.id.into()));
                    }
                    if favorite {
                        actions.push(AppAction::ToggleFavorite {
                            tool_id: descriptor.id.into(),
                        });
                    }
                }
            });
        } else {
            for descriptor in &descriptors {
                let (open, favorite) = home_tool_card(
                    ui,
                    descriptor,
                    self.settings
                        .favorite_tools
                        .iter()
                        .any(|id| id == descriptor.id),
                );
                if open {
                    actions.push(AppAction::NavigateTo(descriptor.id.into()));
                }
                if favorite {
                    actions.push(AppAction::ToggleFavorite {
                        tool_id: descriptor.id.into(),
                    });
                }
                ui.add_space(ui::SPACE_12);
            }
        }
        ui.add_space(32.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("最近使用").strong());
            if !self.settings.recent_tools.is_empty() && ui.small_button("清除").clicked() {
                actions.push(AppAction::ClearRecents);
            }
        });
        ui.add_space(ui::SPACE_8);
        if self.settings.recent_tools.is_empty() {
            ui.label(
                RichText::new("最近打开的工具会显示在这里。").color(ui.visuals().weak_text_color()),
            );
        } else {
            for id in &self.settings.recent_tools {
                if let Some(descriptor) = descriptors.iter().find(|descriptor| descriptor.id == id)
                    && ui.link(descriptor.name).clicked()
                {
                    actions.push(AppAction::NavigateTo(descriptor.id.into()));
                }
            }
        }
        actions
    }

    fn render_settings(&mut self, ui: &mut egui::Ui) -> Vec<AppAction> {
        let mut actions = Vec::new();
        ui::page_heading(ui, "设置", "设置仅保存在当前 Windows 用户的本地配置目录。");
        ui.add_space(ui::SPACE_24);
        ui::card(ui, |ui| {
            ui.label(RichText::new("外观").strong());
            ui.add_space(ui::SPACE_8);
            let before_theme = self.settings.theme;
            egui::ComboBox::from_id_salt("theme-preference")
                .selected_text(self.settings.theme.label())
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut self.settings.theme,
                        ThemePreference::System,
                        "跟随系统",
                    );
                    ui.selectable_value(&mut self.settings.theme, ThemePreference::Light, "浅色");
                    ui.selectable_value(&mut self.settings.theme, ThemePreference::Dark, "深色");
                });
            if before_theme != self.settings.theme {
                apply_theme(ui.ctx(), self.settings.theme);
                self.persist_settings();
            }
        });
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            ui.label(RichText::new("资源管理器右键菜单").strong());
            ui.label(
                RichText::new("在文件右键菜单中提供“Windows Toolbox -> 查看文件占用”。")
                    .color(ui.visuals().weak_text_color()),
            );
            let registered = shell_context_menu::is_context_menu_registered();
            ui.horizontal(|ui| {
                let mut enabled = registered;
                if ui
                    .checkbox(&mut enabled, "启用 Windows Toolbox 右键菜单")
                    .changed()
                {
                    actions.push(AppAction::ToggleContextMenu { enabled });
                }
                ui.label(if registered {
                    "状态：已注册"
                } else {
                    "状态：未注册"
                });
            });
            ui.horizontal(|ui| {
                if ui::primary_button(ui, "重新注册").clicked() {
                    actions.push(AppAction::ToggleContextMenu { enabled: true });
                }
                if ui
                    .add_enabled(registered, egui::Button::new("移除"))
                    .clicked()
                {
                    actions.push(AppAction::ToggleContextMenu { enabled: false });
                }
            });
        });
        ui.add_space(ui::SPACE_16);
        ui::card(ui, |ui| {
            ui.label(RichText::new("诊断").strong());
            ui.label(
                RichText::new("启动失败日志保存在本地配置目录，最多保留三份。")
                    .color(ui.visuals().weak_text_color()),
            );
            let directory = crate::diagnostics::diagnostic_directory();
            if ui
                .add_enabled(directory.is_some(), egui::Button::new("打开诊断目录"))
                .clicked()
                && let Some(directory) = directory
            {
                actions.push(AppAction::OpenDirectory(directory));
            }
        });
        actions
    }

    fn render_about(&self, ui: &mut egui::Ui) -> Vec<AppAction> {
        ui::page_heading(
            ui,
            "关于 Windows Toolbox",
            "V0.2.0 · Windows 原生系统诊断工具集",
        );
        ui.add_space(ui::SPACE_24);
        ui::card(ui, |ui| {
            ui.label("Windows Toolbox 面向 Windows 用户和开发者，提供常用系统状态诊断能力。");
            ui.add_space(ui::SPACE_12);
            ui.label(RichText::new("当前内建工具").strong());
            ui.add_space(ui::SPACE_8);
            ui.label("文件占用 · 通过 Restart Manager 查看锁定文件的进程");
            ui.label("端口占用 · 通过 IP Helper 查看 TCP / UDP 端点");
            ui.label("进程关系 · 通过 Toolhelp 查看父进程链、直接子进程与基础详情");
            ui.label("DNS 查询 · 系统 DNS 记录解析");
            ui.label("Ping · 主机响应与延迟测试");
            ui.label("TCP 端口测试 · TCP 握手与地址族诊断");
            ui.add_space(ui::SPACE_16);
            ui.label(
                RichText::new("默认以普通用户权限运行；受限进程的路径或操作可能需要管理员权限。")
                    .color(ui.visuals().weak_text_color()),
            );
        });
        Vec::new()
    }

    fn handle_actions(&mut self, context: &egui::Context, actions: Vec<AppAction>) {
        for action in actions {
            match action {
                AppAction::NavigateTo(target) => self.navigate_to(&target),
                AppAction::InvokeTool(invocation) => self.apply_invocation(context, invocation),
                AppAction::PickFileForLocks => match pick_file() {
                    Ok(Some(path)) => {
                        self.apply_invocation(context, ToolInvocation::file_lock(path))
                    }
                    Ok(None) => {}
                    Err(error) => self.set_error_notice(error),
                },
                AppAction::ExportPortsCsv { content } => match save_csv(&content) {
                    Ok(Some(path)) => self.set_notice(
                        format!("CSV 已导出到 {}", path.display()),
                        NoticeTone::Success,
                    ),
                    Ok(None) => {}
                    Err(error) => self.set_error_notice(error),
                },
                AppAction::RunDns {
                    host,
                    record_type,
                    bypass_cache,
                } => self.dispatch_task(TaskRequest::DnsLookup {
                    host,
                    record_type,
                    bypass_cache,
                }),
                AppAction::RunPing {
                    host,
                    count,
                    timeout_ms,
                    payload_size,
                    family,
                } => self.dispatch_task(TaskRequest::Ping {
                    host,
                    count,
                    timeout_ms,
                    payload_size,
                    family,
                }),
                AppAction::RunTcpProbe {
                    host,
                    port,
                    timeout_ms,
                } => self.dispatch_task(TaskRequest::TcpProbe {
                    host,
                    port,
                    timeout_ms,
                }),
                AppAction::QueryFileLocks { path } => self.dispatch_file_locks(path),
                AppAction::RefreshPorts => self.dispatch_ports(),
                AppAction::InspectProcess { pid } => self.dispatch_process(pid),
                AppAction::CopyText(text) => context.copy_text(text),
                AppAction::OpenFileLocation(path) => match open_file_location(&path) {
                    Ok(()) => self.set_notice("已在资源管理器中打开文件位置", NoticeTone::Success),
                    Err(error) => self.set_error_notice(error),
                },
                AppAction::OpenDirectory(path) => match open_directory(&path) {
                    Ok(()) => self.set_notice("已打开诊断目录", NoticeTone::Success),
                    Err(error) => self.set_error_notice(error),
                },
                AppAction::RequestTerminateProcess(process) => {
                    self.pending_termination = Some(process)
                }
                AppAction::TerminateProcess { pid } => self.dispatch_termination(pid),
                AppAction::ToggleContextMenu { enabled } => self.toggle_context_menu(enabled),
                AppAction::ToggleFavorite { tool_id } => self.toggle_favorite(&tool_id),
                AppAction::ClearRecents => {
                    self.settings.recent_tools.clear();
                    self.persist_settings();
                }
            }
        }
    }

    fn navigate_to(&mut self, target: &str) {
        self.page = match target {
            "home" => Page::Home,
            "settings" => Page::Settings,
            "about" => Page::About,
            tool_id if self.registry.get(tool_id).is_some() => {
                self.record_recent_tool(tool_id);
                Page::Tool(tool_id.to_owned())
            }
            _ => Page::Home,
        };
    }

    fn apply_invocation(&mut self, context: &egui::Context, invocation: ToolInvocation) {
        // activate 已在接收队列阶段触发 Visible/Minimized/Focus，仅用于恢复窗口，不改变页面状态。
        if invocation.is_activate() {
            return;
        }
        let tool_id = invocation.tool_id.clone();
        if self.registry.get(&tool_id).is_none() {
            self.set_notice("收到未知工具调用", NoticeTone::Danger);
            return;
        }
        self.navigate_to(&tool_id);
        let actions = self
            .registry
            .get_mut(&tool_id)
            .map(|tool| tool.handle_invocation(invocation.payload))
            .unwrap_or_default();
        self.handle_actions(context, actions);
    }

    fn dispatch_file_locks(&mut self, path: PathBuf) {
        self.dispatch_task(TaskRequest::FileLocks { path });
    }

    fn dispatch_ports(&mut self) {
        self.dispatch_task(TaskRequest::Ports);
    }

    fn dispatch_process(&mut self, pid: u32) {
        self.navigate_to("process-inspector");
        self.dispatch_task(TaskRequest::Process { pid });
    }

    fn toggle_context_menu(&mut self, enabled: bool) {
        let result = if enabled {
            std::env::current_exe()
                .map_err(|error| AppError::ShellRegistrationFailed(error.to_string()))
                .and_then(|path| shell_context_menu::register_context_menu(path.as_os_str()))
        } else {
            shell_context_menu::unregister_context_menu()
        };
        match result {
            Ok(()) => {
                self.settings.context_menu_enabled = enabled;
                self.persist_settings();
                self.set_notice(
                    if enabled {
                        "资源管理器右键菜单已注册"
                    } else {
                        "资源管理器右键菜单已移除"
                    },
                    NoticeTone::Success,
                );
            }
            Err(error) => self.set_error_notice(error),
        }
    }

    fn render_termination_dialog(&mut self, context: &egui::Context) -> Vec<AppAction> {
        let Some(process) = self.pending_termination.clone() else {
            return Vec::new();
        };
        let mut open = true;
        let mut confirm = false;
        let mut cancel = false;
        egui::Window::new("确认结束进程")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(context, |ui| {
                ui.label(RichText::new("确定要结束以下进程吗？").strong());
                ui.add_space(8.0);
                ui.label(format!("{}  (PID {})", process.name, process.pid));
                ui.add_space(8.0);
                ui.label(RichText::new("这可能导致未保存的数据丢失。").color(ui::danger_text(ui)));
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if ui.button("取消").clicked() {
                        cancel = true;
                    }
                    if ui::danger_button(ui, "结束进程").clicked() {
                        confirm = true;
                    }
                });
            });
        if confirm {
            self.pending_termination = None;
            return vec![AppAction::TerminateProcess { pid: process.pid }];
        }
        if cancel || !open {
            self.pending_termination = None;
        }
        Vec::new()
    }

    fn dispatch_termination(&mut self, pid: u32) {
        self.dispatch_task(TaskRequest::TerminateProcess { pid });
    }

    fn dispatch_task(&mut self, request: TaskRequest) {
        let tool_id = request.target_tool_id();
        match self.task_dispatcher.dispatch(request) {
            Ok(request_id) => {
                self.latest_requests.insert(tool_id, request_id);
                if let Some(tool) = self.registry.get_mut(tool_id) {
                    tool.set_busy(true);
                }
            }
            Err(error) => {
                let still_pending = self.latest_requests.contains_key(tool_id);
                if let Some(tool) = self.registry.get_mut(tool_id) {
                    tool.set_busy(still_pending);
                }
                self.set_error_notice(error);
            }
        }
    }

    fn record_recent_tool(&mut self, tool_id: &str) {
        self.settings
            .recent_tools
            .retain(|existing| existing != tool_id);
        self.settings.recent_tools.insert(0, tool_id.to_owned());
        self.settings.recent_tools.truncate(8);
        self.persist_settings();
    }

    fn toggle_favorite(&mut self, tool_id: &str) {
        if self.settings.favorite_tools.iter().any(|id| id == tool_id) {
            self.settings.favorite_tools.retain(|id| id != tool_id);
        } else if self.registry.get(tool_id).is_some() {
            self.settings.favorite_tools.push(tool_id.to_owned());
        }
        self.persist_settings();
    }

    fn persist_settings(&mut self) {
        normalize_favorite_tools(
            &self.registry.descriptors(),
            &mut self.settings.favorite_tools,
        );
        if let Err(error) = self.settings_store.save(&self.settings) {
            self.set_error_notice(error);
        }
    }

    fn set_notice(&mut self, message: impl Into<String>, tone: NoticeTone) {
        self.notice = Some(Notice {
            message: message.into(),
            tone,
            expires_at: Instant::now() + Duration::from_secs(4),
        });
    }

    fn set_error_notice(&mut self, error: AppError) {
        log_runtime_error(&error);
        let (title, detail) = error.user_message();
        self.set_notice(format!("{title}：{detail}"), NoticeTone::Danger);
    }

    fn render_notice(&mut self, context: &egui::Context) {
        let Some(notice) = &self.notice else {
            return;
        };
        if Instant::now() >= notice.expires_at {
            self.notice = None;
            return;
        }
        egui::Area::new("toolbox-notice".into())
            .anchor(egui::Align2::RIGHT_BOTTOM, [-18.0, -18.0])
            .show(context, |ui| {
                ui::compact_card(ui, |ui| {
                    let color = match notice.tone {
                        NoticeTone::Success => ui::success_text(ui),
                        NoticeTone::Danger => ui::danger_text(ui),
                    };
                    ui.label(RichText::new(&notice.message).color(color));
                });
            });
    }
}

fn log_runtime_error(error: &AppError) {
    tracing::error!(error_kind = error.diagnostic_label(), "运行操作失败");
}

/// 仅接受同工具最新请求的事件；旧终态既不路由，也不能清除新请求的忙碌状态依据。
fn accept_task_event(
    latest_requests: &mut HashMap<&'static str, RequestId>,
    tool_id: &'static str,
    request_id: RequestId,
    is_finished: bool,
) -> bool {
    if latest_requests.get(tool_id).copied() != Some(request_id) {
        return false;
    }
    if is_finished {
        latest_requests.remove(tool_id);
    }
    true
}

impl eframe::App for ToolboxApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_background_messages(context);
        let pending = std::mem::take(&mut self.pending_invocations);
        for invocation in pending {
            self.apply_invocation(context, invocation);
        }
        for dropped in context.input(|input| input.raw.dropped_files.clone()) {
            if let Some(path) = dropped.path {
                self.apply_invocation(context, ToolInvocation::file_lock(path));
            }
        }

        let scheduled_actions = self.registry.poll_actions(Instant::now());
        self.handle_actions(context, scheduled_actions);
        let sidebar_actions = self.render_sidebar(context);
        self.handle_actions(context, sidebar_actions);
        let content_actions = self.render_content(context);
        self.handle_actions(context, content_actions);

        let dialog_actions = self.render_termination_dialog(context);
        self.handle_actions(context, dialog_actions);
        self.render_notice(context);
        context.request_repaint_after(Duration::from_millis(100));
    }
}

pub fn native_options(settings: &AppSettings, persistence_path: PathBuf) -> eframe::NativeOptions {
    let _ = settings;
    eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([960.0, 620.0])
            .with_clamp_size_to_monitor_size(true),
        persist_window: true,
        persistence_path: Some(persistence_path),
        ..Default::default()
    }
}

fn nav_section_label(ui: &mut egui::Ui, label: &str) {
    ui.label(
        RichText::new(label)
            .size(12.0)
            .color(ui.visuals().weak_text_color()),
    );
}

fn nav_text_button(ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
    let available = ui.available_width();
    let (response, painter) =
        ui.allocate_painter(egui::vec2(available, 34.0), egui::Sense::click());
    let fill = if selected {
        ui::accent(ui).gamma_multiply(0.18)
    } else if response.hovered() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(response.rect, egui::CornerRadius::same(6), fill);
    if response.has_focus() {
        painter.rect_stroke(
            response.rect.shrink(1.0),
            egui::CornerRadius::same(6),
            egui::Stroke::new(2.0_f32, ui::accent(ui)),
            egui::StrokeKind::Middle,
        );
    }
    if selected {
        painter.rect_filled(
            egui::Rect::from_min_size(
                response.rect.left_top(),
                egui::vec2(3.0, response.rect.height()),
            ),
            egui::CornerRadius::same(2),
            ui::accent(ui),
        );
    }
    painter.text(
        response.rect.left_center() + egui::vec2(12.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        ui.visuals().text_color(),
    );
    if response.clicked() {
        response.request_focus();
    }
    response.clicked()
        || (response.has_focus()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            }))
}

fn tool_nav_button(ui: &mut egui::Ui, descriptor: &ToolDescriptor, selected: bool) -> bool {
    let available = ui.available_width();
    let (response, painter) =
        ui.allocate_painter(egui::vec2(available, 38.0), egui::Sense::click());
    let fill = if selected {
        ui::accent(ui).gamma_multiply(0.18)
    } else if response.hovered() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(response.rect, egui::CornerRadius::same(6), fill);
    if response.has_focus() {
        painter.rect_stroke(
            response.rect.shrink(1.0),
            egui::CornerRadius::same(6),
            egui::Stroke::new(2.0_f32, ui::accent(ui)),
            egui::StrokeKind::Middle,
        );
    }
    if selected {
        painter.rect_filled(
            egui::Rect::from_min_size(
                response.rect.left_top(),
                egui::vec2(3.0, response.rect.height()),
            ),
            egui::CornerRadius::same(2),
            ui::accent(ui),
        );
    }
    let icon_rect = egui::Rect::from_min_size(
        response.rect.left_top() + egui::vec2(10.0, 7.0),
        egui::vec2(24.0, 24.0),
    );
    ui::paint_tool_icon(
        &painter,
        icon_rect,
        descriptor.icon,
        if selected {
            ui::accent(ui)
        } else {
            ui.visuals().weak_text_color()
        },
    );
    painter.text(
        response.rect.left_center() + egui::vec2(44.0, 0.0),
        egui::Align2::LEFT_CENTER,
        descriptor.name,
        egui::FontId::proportional(14.0),
        ui.visuals().text_color(),
    );
    if response.clicked() {
        response.request_focus();
    }
    response.clicked()
        || (response.has_focus()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            }))
}

fn home_tool_card(ui: &mut egui::Ui, descriptor: &ToolDescriptor, favorite: bool) -> (bool, bool) {
    let width = ui.available_width().min(520.0);
    let (response, painter) = ui.allocate_painter(egui::vec2(width, 112.0), egui::Sense::click());
    let painter = painter.with_clip_rect(response.rect);
    let fill = if response.hovered() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        ui.visuals().window_fill
    };
    let stroke = if response.hovered() {
        egui::Stroke::new(1.0_f32, ui::accent(ui))
    } else {
        ui.visuals().widgets.inactive.bg_stroke
    };
    painter.rect(
        response.rect,
        egui::CornerRadius::same(8),
        fill,
        stroke,
        egui::StrokeKind::Middle,
    );
    if response.has_focus() {
        painter.rect_stroke(
            response.rect.shrink(1.0),
            egui::CornerRadius::same(8),
            egui::Stroke::new(2.0_f32, ui::accent(ui)),
            egui::StrokeKind::Middle,
        );
    }
    let content = response.rect.shrink(16.0);
    ui::paint_tool_icon(
        &painter,
        egui::Rect::from_min_size(content.left_top(), egui::vec2(34.0, 34.0)),
        descriptor.icon,
        ui::accent(ui),
    );
    painter.text(
        content.left_top() + egui::vec2(46.0, 3.0),
        egui::Align2::LEFT_TOP,
        descriptor.name,
        egui::FontId::proportional(16.0),
        ui.visuals().text_color(),
    );
    let favorite_rect = egui::Rect::from_min_size(
        response.rect.right_top() - egui::vec2(34.0, -8.0),
        egui::vec2(26.0, 26.0),
    );
    let favorite_response = ui.interact(
        favorite_rect,
        ui.id().with(("favorite", descriptor.id)),
        egui::Sense::click(),
    );
    favorite_response.clone().on_hover_text(if favorite {
        "取消收藏"
    } else {
        "收藏工具"
    });
    if favorite_response.has_focus() {
        painter.rect_stroke(
            favorite_rect.shrink(1.0),
            egui::CornerRadius::same(6),
            egui::Stroke::new(2.0_f32, ui::accent(ui)),
            egui::StrokeKind::Middle,
        );
    }
    painter.text(
        favorite_rect.center(),
        egui::Align2::CENTER_CENTER,
        if favorite { "★" } else { "☆" },
        egui::FontId::proportional(18.0),
        if favorite {
            ui::accent(ui)
        } else {
            ui.visuals().weak_text_color()
        },
    );
    painter.text(
        content.left_top() + egui::vec2(46.0, 31.0),
        egui::Align2::LEFT_TOP,
        descriptor.description,
        egui::FontId::proportional(14.0),
        ui.visuals().weak_text_color(),
    );
    painter.text(
        content.right_bottom(),
        egui::Align2::RIGHT_BOTTOM,
        "打开 →",
        egui::FontId::proportional(12.0),
        ui::accent(ui),
    );
    if response.clicked() {
        response.request_focus();
    }
    let open = response.clicked()
        || (response.has_focus()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            }));
    if favorite_response.clicked() {
        favorite_response.request_focus();
    }
    let toggle_favorite = favorite_response.clicked()
        || (favorite_response.has_focus()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            }));
    (open && !toggle_favorite, toggle_favorite)
}

/// 按当前注册顺序保留收藏项，清除旧版本遗留的无效或重复工具标识。
fn normalize_favorite_tools(descriptors: &[ToolDescriptor], favorite_tools: &mut Vec<String>) {
    let selected: HashSet<_> = favorite_tools.iter().map(String::as_str).collect();
    *favorite_tools = descriptors
        .iter()
        .filter(|descriptor| selected.contains(descriptor.id))
        .map(|descriptor| descriptor.id.to_owned())
        .collect();
}

fn apply_theme(context: &egui::Context, preference: ThemePreference) {
    let preference = match preference {
        ThemePreference::System => egui::ThemePreference::System,
        ThemePreference::Light => egui::ThemePreference::Light,
        ThemePreference::Dark => egui::ThemePreference::Dark,
    };
    context.set_theme(preference);
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{accept_task_event, normalize_favorite_tools};
    use crate::tools::build_registry;

    #[test]
    fn favorite_tools_are_deduplicated_filtered_and_registry_ordered() {
        let descriptors = build_registry().descriptors();
        let mut favorites = vec![
            "process-inspector".to_owned(),
            "missing-tool".to_owned(),
            "file-lock".to_owned(),
            "process-inspector".to_owned(),
        ];

        normalize_favorite_tools(&descriptors, &mut favorites);

        assert_eq!(favorites, ["file-lock", "process-inspector"]);
    }

    #[test]
    fn stale_finished_event_does_not_clear_or_route_latest_request() {
        let mut latest = HashMap::from([("port-inspector", 2)]);
        assert!(!accept_task_event(&mut latest, "port-inspector", 1, true));
        assert_eq!(latest.get("port-inspector"), Some(&2));
        assert!(accept_task_event(&mut latest, "port-inspector", 2, true));
        assert!(!latest.contains_key("port-inspector"));
    }
}
