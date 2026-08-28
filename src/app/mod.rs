//! Windows Toolbox 的 GUI 外壳。
//! 该模块拥有 HUD 顶栏、分类折叠手风琴侧边栏、主题管理、设置、后台任务协作与 Tool Invocation 路由。

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::mpsc::Receiver,
    time::{Duration, Instant},
};

use eframe::{
    egui,
    egui::{Color32, RichText, Vec2},
};

use crate::{
    core::{
        actions::AppAction,
        communication::CommunicationDispatcher,
        invocation::ToolInvocation,
        worker::{RequestId, TaskDispatcher, TaskEvent, TaskEventEnvelope, TaskRequest},
    },
    model::{AppError, ProcessSummary},
    platform::windows::{
        open_directory, open_file_location, pick_file, save_csv, save_log, shell_context_menu,
    },
    settings::{AppSettings, SettingsStore, ThemePreference},
    tools::{ToolCategory, ToolDescriptor, ToolRegistry, ToolUiContext, build_registry},
    ui,
};

mod actions;
mod content;
mod layout;
mod navigation;
mod overlay;
mod state;

use state::{Notice, NoticeTone, Page};

/// Tool Host：统一协调 Tool Registry、UI、后台任务和系统集成。
pub struct ToolboxApp {
    registry: ToolRegistry,
    page: Page,
    search: String,
    active_category: ToolCategory,
    compact_tool_nav_open: bool,
    settings_store: SettingsStore,
    settings: AppSettings,
    task_dispatcher: TaskDispatcher,
    task_receiver: Receiver<TaskEventEnvelope>,
    communication_dispatcher: CommunicationDispatcher,
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
        navigation::normalize_favorite_tools(&registry.descriptors(), &mut settings.favorite_tools);
        if settings.favorite_tools != favorites_before {
            let _ = settings_store.save(&settings);
        }
        ui::install_windows_fonts(&creation_context.egui_ctx);
        ui::configure_styles(&creation_context.egui_ctx);
        apply_theme(&creation_context.egui_ctx, settings.theme);
        let (task_dispatcher, task_receiver) = TaskDispatcher::new()?;
        let communication_dispatcher = CommunicationDispatcher::new();
        Ok(Self {
            registry,
            page: Page::Home,
            search: String::new(),
            active_category: ToolCategory::Network,
            compact_tool_nav_open: false,
            settings_store,
            settings,
            task_dispatcher,
            task_receiver,
            communication_dispatcher,
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
            if !actions::accept_task_event(
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
                TaskEvent::MtrProgress(progress) => {
                    if let Some(tool) = self.registry.get_mut(envelope.target_tool_id) {
                        tool.handle_mtr_progress(progress);
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

        for envelope in self.communication_dispatcher.drain_events() {
            if let Some(tool) = self.registry.get_mut(envelope.kind.tool_id()) {
                tool.handle_communication_event(envelope);
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

    /// 绘制顶部全局栏，集中放置品牌、搜索与全局入口。
    fn render_top_hud(&mut self, context: &egui::Context) -> Vec<AppAction> {
        let mut actions = Vec::new();
        let palette = ui::theme_palette(if context.style().visuals.dark_mode {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });

        egui::TopBottomPanel::top("toolbox-top-hud")
            .exact_height(layout::TOP_BAR_HEIGHT)
            .frame(
                egui::Frame::new()
                    .fill(palette.panel)
                    .stroke(egui::Stroke::new(1.0_f32, palette.border_subtle))
                    .inner_margin(egui::Margin::symmetric(12, 7)),
            )
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    let (logo_rect, _) =
                        ui.allocate_exact_size(Vec2::splat(28.0), egui::Sense::hover());
                    ui::paint_app_icon(
                        ui.painter(),
                        logo_rect.shrink(5.0),
                        ui::AppIcon::Developer,
                        palette.accent,
                    );
                    ui.add_space(ui::SPACE_4);
                    ui.label(
                        RichText::new("ToolDeck")
                            .strong()
                            .size(16.0)
                            .color(palette.text),
                    );
                    ui.add_space(ui::SPACE_12);
                    ui.separator();
                    ui.add_space(ui::SPACE_12);
                    let search_width = (ui.available_width() - 132.0).clamp(180.0, 440.0);
                    ui.add_sized(
                        [search_width, ui::CONTROL_HEIGHT],
                        ui::text_input(&mut self.search, "搜索工具"),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::icon_button(
                            ui,
                            ui::AppIcon::About,
                            "关于与架构",
                            self.page == Page::About,
                        )
                        .clicked()
                        {
                            actions.push(AppAction::NavigateTo("about".into()));
                        }
                        if ui::icon_button(
                            ui,
                            ui::AppIcon::Settings,
                            "系统设置",
                            self.page == Page::Settings,
                        )
                        .clicked()
                        {
                            actions.push(AppAction::NavigateTo("settings".into()));
                        }
                        let next_theme = match self.settings.theme {
                            ThemePreference::Dark => ThemePreference::Light,
                            ThemePreference::Light => ThemePreference::System,
                            ThemePreference::System => ThemePreference::Dark,
                        };
                        let theme_name = match self.settings.theme {
                            ThemePreference::Dark => "当前为深色主题，点击切换",
                            ThemePreference::Light => "当前为浅色主题，点击切换",
                            ThemePreference::System => "当前跟随系统主题，点击切换",
                        };
                        if ui::icon_button(ui, ui::AppIcon::Theme, theme_name, false).clicked() {
                            self.settings.theme = next_theme;
                            apply_theme(ui.ctx(), self.settings.theme);
                            self.persist_settings();
                        }
                    });
                });
            });

        actions
    }

    /// 绘制分类图标栏与当前分类工具栏；窄窗口仅永久保留分类栏。
    fn render_sidebar(&mut self, context: &egui::Context) -> Vec<AppAction> {
        let descriptors = if self.search.trim().is_empty() {
            Vec::new()
        } else {
            self.registry.search(&self.search)
        };
        let categories = self.registry.categories();
        let nav_layout = layout::navigation_layout(context.screen_rect().width());
        let mut actions = Vec::new();

        egui::SidePanel::left("toolbox-category-rail")
            .resizable(false)
            .exact_width(layout::CATEGORY_RAIL_WIDTH)
            .show(context, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(ui::SPACE_8);
                    if ui::icon_button_sized(
                        ui,
                        ui::AppIcon::Home,
                        "首页",
                        self.page == Page::Home,
                        layout::CATEGORY_ROW_HEIGHT,
                    )
                    .clicked()
                    {
                        actions.push(AppAction::NavigateTo("home".into()));
                    }
                    ui.add_space(ui::SPACE_8);
                    for category in categories {
                        let selected =
                            self.active_category == category && matches!(self.page, Page::Tool(_));
                        if ui::icon_button_sized(
                            ui,
                            category_app_icon(category),
                            category.label(),
                            selected,
                            layout::CATEGORY_ROW_HEIGHT,
                        )
                        .clicked()
                        {
                            if self.active_category == category
                                && nav_layout == layout::NavigationLayout::Compact
                            {
                                self.compact_tool_nav_open = !self.compact_tool_nav_open;
                            } else {
                                self.active_category = category;
                                self.compact_tool_nav_open = true;
                            }
                        }
                    }
                });
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                    ui.add_space(ui::SPACE_8);
                    if ui::icon_button_sized(
                        ui,
                        ui::AppIcon::About,
                        "关于与架构",
                        self.page == Page::About,
                        layout::CATEGORY_ROW_HEIGHT,
                    )
                    .clicked()
                    {
                        actions.push(AppAction::NavigateTo("about".into()));
                    }
                    if ui::icon_button_sized(
                        ui,
                        ui::AppIcon::Settings,
                        "系统设置",
                        self.page == Page::Settings,
                        layout::CATEGORY_ROW_HEIGHT,
                    )
                    .clicked()
                    {
                        actions.push(AppAction::NavigateTo("settings".into()));
                    }
                });
            });

        let show_tool_nav = nav_layout == layout::NavigationLayout::Wide
            || self.compact_tool_nav_open
            || !self.search.trim().is_empty();
        if show_tool_nav {
            egui::SidePanel::left("toolbox-tool-navigation")
                .resizable(false)
                .exact_width(layout::TOOL_NAV_WIDTH)
                .frame(
                    egui::Frame::side_top_panel(context.style().as_ref())
                        .fill(
                            ui::theme_palette(if context.style().visuals.dark_mode {
                                egui::Theme::Dark
                            } else {
                                egui::Theme::Light
                            })
                            .surface,
                        )
                        .inner_margin(egui::Margin::symmetric(12, 12)),
                )
                .show(context, |ui| {
                    ui.horizontal(|ui| {
                        let title = if self.search.trim().is_empty() {
                            format!("{}工具", self.active_category.label())
                        } else {
                            "搜索结果".to_owned()
                        };
                        ui.label(RichText::new(title).strong().size(15.0));
                        if nav_layout == layout::NavigationLayout::Compact {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui::icon_button(ui, ui::AppIcon::Back, "收起工具导航", false)
                                        .clicked()
                                    {
                                        self.compact_tool_nav_open = false;
                                    }
                                },
                            );
                        }
                    });
                    ui.add_space(ui::SPACE_8);
                    ui.separator();
                    ui.add_space(ui::SPACE_8);
                    if nav_text_button(ui, "首页 / 工具工作台", self.page == Page::Home) {
                        actions.push(AppAction::NavigateTo("home".into()));
                    }
                    ui.add_space(ui::SPACE_8);
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let shown = if self.search.trim().is_empty() {
                                self.registry.in_category(self.active_category)
                            } else {
                                descriptors
                            };
                            if shown.is_empty() {
                                ui.label(
                                    RichText::new("没有匹配的工具")
                                        .size(12.5)
                                        .color(ui.visuals().weak_text_color()),
                                );
                            }
                            for descriptor in shown {
                                if tool_nav_button(
                                    ui,
                                    &descriptor,
                                    self.page == Page::Tool(descriptor.id.into()),
                                ) {
                                    actions.push(AppAction::NavigateTo(descriptor.id.into()));
                                    self.compact_tool_nav_open = false;
                                }
                            }
                        });
                });
        }
        actions
    }

    fn render_content(&mut self, context: &egui::Context) -> Vec<AppAction> {
        let page = self.page.clone();
        egui::CentralPanel::default()
            .frame(
                egui::Frame::central_panel(context.style().as_ref())
                    .inner_margin(egui::Margin::symmetric(ui::PAGE_PADDING as i8, 16)),
            )
            .show(context, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(content::MAIN_SCROLL_ID)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let content_w = layout::content_width(ui.available_width());
                        let available_h = ui.available_height().max(500.0);
                        ui.with_layout(egui::Layout::top_down(egui::Align::Center), |ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(content_w, available_h),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| match page {
                                    Page::Home => self.render_home(ui),
                                    Page::Tool(id) => self
                                        .registry
                                        .get_mut(&id)
                                        .map(|tool| tool.ui(ui, ToolUiContext))
                                        .unwrap_or_else(|| {
                                            vec![AppAction::NavigateTo("home".into())]
                                        }),
                                    Page::Settings => self.render_settings(ui),
                                    Page::About => self.render_about(ui),
                                },
                            )
                            .inner
                        })
                        .inner
                    })
                    .inner
            })
            .inner
    }

    /// 绘制紧凑工具工作台，突出收藏、最近使用和完整工具索引。
    fn render_home(&mut self, ui: &mut egui::Ui) -> Vec<AppAction> {
        let mut actions = Vec::new();
        let palette = ui::palette_for_ui(ui);
        let descriptors = self.registry.descriptors();
        let favorites =
            navigation::favorite_descriptors(&descriptors, &self.settings.favorite_tools);

        ui::page_heading(ui, "工具工作台", "系统与网络工具集中入口");
        ui.add_space(ui::SPACE_16);

        egui::Frame::new()
            .fill(palette.surface)
            .stroke(egui::Stroke::new(1.0_f32, palette.border_subtle))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.columns(3, |columns| {
                    home_summary_item(
                        &mut columns[0],
                        "内建工具",
                        descriptors.len(),
                        palette.accent,
                    );
                    home_summary_item(
                        &mut columns[1],
                        "收藏",
                        favorites.len(),
                        palette.warning_text,
                    );
                    home_summary_item(
                        &mut columns[2],
                        "最近使用",
                        self.settings.recent_tools.len(),
                        palette.success_text,
                    );
                });
            });

        ui.add_space(ui::SPACE_16);
        ui.label(RichText::new("收藏工具").strong().size(14.5));
        ui.add_space(ui::SPACE_8);
        if favorites.is_empty() {
            ui.label(RichText::new("尚未收藏工具").size(12.5).color(palette.weak));
        } else {
            egui::Frame::new()
                .stroke(egui::Stroke::new(1.0_f32, palette.border_subtle))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::same(4))
                .show(ui, |ui| {
                    for (index, descriptor) in favorites.iter().enumerate() {
                        let (open, favorite) = home_tool_row(ui, descriptor, true, index % 2 == 1);
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
        }

        ui.add_space(ui::SPACE_16);
        ui.horizontal(|ui| {
            ui.label(RichText::new("最近使用").strong().size(14.5));
            if !self.settings.recent_tools.is_empty()
                && ui::icon_button(ui, ui::AppIcon::Clear, "清空最近使用", false).clicked()
            {
                actions.push(AppAction::ClearRecents);
            }
        });
        ui.add_space(ui::SPACE_8);
        if self.settings.recent_tools.is_empty() {
            ui.label(
                RichText::new("打开过的工具会显示在这里")
                    .size(12.5)
                    .color(palette.weak),
            );
        } else {
            ui.horizontal_wrapped(|ui| {
                for id in &self.settings.recent_tools {
                    if let Some(descriptor) = descriptors.iter().find(|item| item.id == id)
                        && ui::secondary_button(ui, descriptor.name).clicked()
                    {
                        actions.push(AppAction::NavigateTo(descriptor.id.into()));
                    }
                }
            });
        }

        ui.add_space(ui::SPACE_20);
        ui.label(RichText::new("全部工具").strong().size(14.5));
        ui.add_space(ui::SPACE_8);
        for category in self.registry.categories() {
            let category_tools = self.registry.in_category(category);
            ui.label(
                RichText::new(category.label())
                    .size(12.0)
                    .strong()
                    .color(palette.weak),
            );
            ui.add_space(ui::SPACE_4);
            egui::Frame::new()
                .stroke(egui::Stroke::new(1.0_f32, palette.border_subtle))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::same(4))
                .show(ui, |ui| {
                    for (index, descriptor) in category_tools.iter().enumerate() {
                        let is_favorite = self
                            .settings
                            .favorite_tools
                            .iter()
                            .any(|id| id == descriptor.id);
                        let (open, favorite) =
                            home_tool_row(ui, descriptor, is_favorite, index % 2 == 1);
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
            ui.add_space(ui::SPACE_12);
        }

        actions
    }

    fn render_settings(&mut self, ui: &mut egui::Ui) -> Vec<AppAction> {
        let mut actions = Vec::new();
        let palette = ui::palette_for_ui(ui);
        ui::page_heading(ui, "系统设置", "外观、系统集成与本地诊断存储");
        ui.add_space(ui::SPACE_16);

        ui::section(ui, |ui| {
            ui.label(RichText::new("外观").strong().size(15.0));
            ui.add_space(ui::SPACE_12);
            let before_theme = self.settings.theme;
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label("主题模式");
                    ui.label(
                        RichText::new("选择应用的明暗外观")
                            .size(12.0)
                            .color(palette.weak),
                    );
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.selectable_value(&mut self.settings.theme, ThemePreference::Dark, "深色");
                    ui.selectable_value(&mut self.settings.theme, ThemePreference::Light, "浅色");
                    ui.selectable_value(
                        &mut self.settings.theme,
                        ThemePreference::System,
                        "跟随系统",
                    );
                });
            });
            if before_theme != self.settings.theme {
                apply_theme(ui.ctx(), self.settings.theme);
                self.persist_settings();
            }
        });

        ui::section(ui, |ui| {
            ui.label(RichText::new("资源管理器集成").strong().size(15.0));
            ui.add_space(ui::SPACE_12);
            ui.label(
                RichText::new("在文件右键菜单中显示 ToolDeck 文件占用入口")
                    .size(12.5)
                    .color(palette.weak),
            );
            ui.add_space(ui::SPACE_8);
            let registered = shell_context_menu::is_context_menu_registered();
            ui.horizontal(|ui| {
                let mut enabled = registered;
                if ui
                    .checkbox(&mut enabled, "启用资源管理器右键菜单")
                    .changed()
                {
                    actions.push(AppAction::ToggleContextMenu { enabled });
                }
                if registered {
                    ui::badge(
                        ui,
                        "已注册",
                        palette.success_text,
                        palette.success_text.gamma_multiply(0.15),
                    );
                } else {
                    ui::badge(ui, "未注册", palette.weak, palette.border_subtle);
                }
            });
            ui.add_space(ui::SPACE_8);
            ui.horizontal(|ui| {
                if ui::secondary_button(ui, "重新注册").clicked() {
                    actions.push(AppAction::ToggleContextMenu { enabled: true });
                }
                if registered && ui::secondary_button(ui, "移除菜单").clicked() {
                    actions.push(AppAction::ToggleContextMenu { enabled: false });
                }
            });
        });

        ui::section(ui, |ui| {
            ui.label(RichText::new("诊断与存储").strong().size(15.0));
            ui.add_space(ui::SPACE_12);
            ui.label(
                RichText::new("诊断与崩溃日志保存在本地目录，自动轮转并保留最近三份")
                    .size(12.5)
                    .color(palette.weak),
            );
            ui.add_space(ui::SPACE_8);
            let directory = crate::diagnostics::diagnostic_directory();
            if let Some(directory) = directory {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new(directory.display().to_string())
                            .monospace()
                            .size(12.0)
                            .color(palette.text),
                    );
                    if ui::secondary_button(ui, "打开目录").clicked() {
                        actions.push(AppAction::OpenDirectory(directory));
                    }
                });
            } else {
                ui.label(RichText::new("诊断目录尚未建立").color(palette.weak));
            }
        });
        actions
    }

    fn render_about(&self, ui: &mut egui::Ui) -> Vec<AppAction> {
        let palette = ui::palette_for_ui(ui);
        ui::page_heading(ui, "关于与架构", "ToolDeck Windows 系统与网络工具工作台");
        ui.add_space(ui::SPACE_16);

        ui::section(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("ToolDeck").strong().size(24.0));
                ui::badge(
                    ui,
                    concat!("v", env!("CARGO_PKG_VERSION")),
                    palette.accent,
                    palette.accent.gamma_multiply(0.12),
                );
            });
            ui.add_space(ui::SPACE_8);
            ui.columns(2, |columns| {
                about_value(&mut columns[0], "运行平台", "Windows 原生应用", palette);
                about_value(&mut columns[0], "界面框架", "Rust + egui", palette);
                about_value(
                    &mut columns[1],
                    "软件许可",
                    env!("CARGO_PKG_LICENSE"),
                    palette,
                );
                about_value(&mut columns[1], "目标架构", std::env::consts::ARCH, palette);
            });
        });

        ui::section(ui, |ui| {
            ui.label(RichText::new("架构边界").strong().size(15.0));
            ui.add_space(ui::SPACE_12);
            ui.columns(5, |columns| {
                architecture_node(&mut columns[0], "UI", "展示与输入", palette);
                architecture_node(&mut columns[1], "App Actions", "命令与状态", palette);
                architecture_node(&mut columns[2], "Worker", "短任务", palette);
                architecture_node(&mut columns[3], "Communication", "持久会话", palette);
                architecture_node(&mut columns[4], "Windows APIs", "系统接口", palette);
            });
            ui.add_space(ui::SPACE_12);
            ui.label(
                RichText::new(
                    "UI 不执行耗时操作；Worker 处理受限短任务；Communication Dispatcher 管理持续通信会话。",
                )
                .size(12.5)
                .color(palette.weak),
            );
        });

        ui.label(RichText::new("能力清单").strong().size(15.0));
        ui.add_space(ui::SPACE_12);
        ui.columns(2, |columns| {
            for (index, category) in self.registry.categories().iter().enumerate() {
                let column = &mut columns[index % 2];
                column.label(
                    RichText::new(category.label())
                        .strong()
                        .size(13.0)
                        .color(palette.accent),
                );
                for descriptor in self.registry.in_category(*category) {
                    column.label(
                        RichText::new(format!("{}  {}", descriptor.name, descriptor.description))
                            .size(12.0)
                            .color(palette.text),
                    );
                }
                column.add_space(ui::SPACE_12);
            }
        });
        ui.add_space(ui::SPACE_16);
        ui.label(
            RichText::new(
                "ToolDeck 默认使用当前用户权限；访问受保护的系统资源时由对应操作返回明确错误。",
            )
            .size(12.0)
            .color(palette.weak),
        );
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
                AppAction::ExportCommunicationLog { content, file_name } => {
                    match save_log(&content, &file_name) {
                        Ok(Some(path)) => self.set_notice(
                            format!("通信记录已导出到 {}", path.display()),
                            NoticeTone::Success,
                        ),
                        Ok(None) => {}
                        Err(error) => self.set_error_notice(error),
                    }
                }
                AppAction::StartCommunication(config) => {
                    if let Err(error) = self.communication_dispatcher.start(config) {
                        self.set_error_notice(error);
                    }
                }
                AppAction::SendCommunication { kind, command } => {
                    if let Err(error) = self.communication_dispatcher.send(kind, command) {
                        self.set_error_notice(error);
                    }
                }
                AppAction::StopCommunication(kind) => {
                    if let Err(error) = self.communication_dispatcher.stop(kind) {
                        self.set_error_notice(error);
                    }
                }
                AppAction::RefreshSerialPorts => self.dispatch_task(TaskRequest::SerialPorts),
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
                AppAction::RunMtr { host, config } => {
                    self.dispatch_task(TaskRequest::Mtr { host, config })
                }
                AppAction::StopMtr => {
                    if let Some(request_id) = self.latest_requests.get("mtr").copied()
                        && self.task_dispatcher.cancel(request_id)
                    {
                        self.set_notice(
                            "已请求停止 MTR，将在当前探测结束后退出",
                            NoticeTone::Success,
                        );
                    }
                }
                AppAction::QueryFileLocks { path } => self.dispatch_file_locks(path),
                AppAction::RefreshPorts => self.dispatch_ports(),
                AppAction::LoadProcessTree => self.dispatch_process_tree(),
                AppAction::InspectProcess { pid } => self.dispatch_process(pid),
                AppAction::InspectProcessCommandLine { pid } => {
                    self.dispatch_process_command_line(pid)
                }
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
                if let Some(descriptor) = self.registry.get(tool_id).map(|tool| tool.descriptor()) {
                    self.active_category = descriptor.category;
                }
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

    fn dispatch_process_tree(&mut self) {
        self.navigate_to("process-inspector");
        self.dispatch_task(TaskRequest::ProcessTree);
    }

    fn dispatch_process_command_line(&mut self, pid: u32) {
        self.dispatch_task(TaskRequest::ProcessCommandLine { pid });
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
                ui.horizontal(|ui| {
                    ui.colored_label(ui::danger_text(ui), "⚠");
                    ui.label(
                        RichText::new("确定要强制结束该进程吗？")
                            .strong()
                            .size(15.0),
                    );
                });
                ui.add_space(ui::SPACE_8);
                ui::card(ui, |ui| {
                    ui.label(RichText::new(&process.name).strong().size(14.0));
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(format!("PID: {}", process.pid))
                            .monospace()
                            .color(ui.visuals().weak_text_color()),
                    );
                    if let Some(path) = &process.exe_path {
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new(path)
                                .monospace()
                                .small()
                                .color(ui.visuals().weak_text_color()),
                        );
                    }
                });
                ui.add_space(ui::SPACE_8);
                ui.label(
                    RichText::new("强制结束进程可能导致未保存的工作丢失或依赖服务异常。")
                        .size(12.5)
                        .color(ui::danger_text(ui)),
                );
                ui.add_space(ui::SPACE_16);
                ui.horizontal(|ui| {
                    if ui::secondary_button(ui, "取消").clicked() {
                        cancel = true;
                    }
                    if ui::danger_button(ui, "确认结束进程").clicked() {
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
        navigation::normalize_favorite_tools(
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
            expires_at: Instant::now() + overlay::notice_duration(tone),
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
            .anchor(egui::Align2::RIGHT_BOTTOM, [-20.0, -20.0])
            .show(context, |ui| {
                ui::compact_card(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (icon, color) = match notice.tone {
                            NoticeTone::Success => ("✓", ui::success_text(ui)),
                            NoticeTone::Danger => ("⚠", ui::danger_text(ui)),
                        };
                        ui.colored_label(color, icon);
                        ui.label(RichText::new(&notice.message).color(color).strong());
                    });
                });
            });
    }
}

fn log_runtime_error(error: &AppError) {
    tracing::error!(error_kind = error.diagnostic_label(), "运行操作失败");
}

/// 仅接受同工具最新请求的事件；旧终态既不路由，也不能清除新请求的忙碌状态依据。
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

        let top_actions = self.render_top_hud(context);
        self.handle_actions(context, top_actions);

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
            .with_inner_size([1200.0, 780.0])
            .with_min_inner_size([980.0, 640.0])
            .with_clamp_size_to_monitor_size(true),
        persist_window: true,
        persistence_path: Some(persistence_path),
        ..Default::default()
    }
}

fn home_summary_item(ui: &mut egui::Ui, label: &str, value: usize, color: Color32) {
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new(value.to_string())
                .strong()
                .size(19.0)
                .color(color),
        );
        ui.label(
            RichText::new(label)
                .size(12.0)
                .color(ui.visuals().weak_text_color()),
        );
    });
}

fn about_value(ui: &mut egui::Ui, label: &str, value: &str, palette: ui::ThemePalette) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(12.0).color(palette.weak));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(RichText::new(value).size(12.5).color(palette.text));
        });
    });
}

fn architecture_node(ui: &mut egui::Ui, title: &str, detail: &str, palette: ui::ThemePalette) {
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0_f32, palette.border))
        .corner_radius(egui::CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(8, 10))
        .show(ui, |ui| {
            ui.set_min_height(52.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(title).strong().size(12.5));
                ui.label(RichText::new(detail).size(11.0).color(palette.weak));
            });
        });
}

fn category_app_icon(category: ToolCategory) -> ui::AppIcon {
    match category {
        ToolCategory::File => ui::AppIcon::File,
        ToolCategory::Network => ui::AppIcon::Network,
        ToolCategory::System => ui::AppIcon::System,
        ToolCategory::Developer => ui::AppIcon::Developer,
        ToolCategory::Text | ToolCategory::Security | ToolCategory::Other => ui::AppIcon::File,
    }
}

fn nav_text_button(ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
    let available = ui.available_width();
    let (response, painter) = ui.allocate_painter(
        egui::vec2(available, layout::TOOL_ROW_HEIGHT),
        egui::Sense::click(),
    );
    let palette = ui::palette_for_ui(ui);
    let fill = if selected {
        palette
            .accent
            .gamma_multiply(if ui.visuals().dark_mode { 0.20 } else { 0.12 })
    } else if response.hovered() {
        palette.hover
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(response.rect, egui::CornerRadius::same(6), fill);
    if response.has_focus() {
        painter.rect_stroke(
            response.rect.shrink(1.0),
            egui::CornerRadius::same(6),
            egui::Stroke::new(1.5_f32, palette.accent),
            egui::StrokeKind::Middle,
        );
    }
    if selected {
        painter.rect_filled(
            egui::Rect::from_min_size(
                response.rect.left_top() + egui::vec2(0.0, 4.0),
                egui::vec2(3.0, response.rect.height() - 8.0),
            ),
            egui::CornerRadius::same(2),
            palette.accent,
        );
    }
    let text_color = if selected {
        palette.accent
    } else {
        palette.text
    };
    let galley = painter.layout_no_wrap(
        label.to_owned(),
        egui::FontId::proportional(13.5),
        text_color,
    );
    painter.galley(
        response.rect.left_center() + egui::vec2(12.0, -galley.size().y * 0.5),
        galley,
        text_color,
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
    let (response, painter) = ui.allocate_painter(
        egui::vec2(available, layout::TOOL_ROW_HEIGHT),
        egui::Sense::click(),
    );
    let palette = ui::palette_for_ui(ui);
    let fill = if selected {
        palette
            .accent
            .gamma_multiply(if ui.visuals().dark_mode { 0.20 } else { 0.12 })
    } else if response.hovered() {
        palette.hover
    } else {
        Color32::TRANSPARENT
    };
    painter.rect(
        response.rect,
        egui::CornerRadius::same(6),
        fill,
        if selected {
            egui::Stroke::new(1.0_f32, palette.accent.gamma_multiply(0.35))
        } else {
            egui::Stroke::NONE
        },
        egui::StrokeKind::Middle,
    );
    if response.has_focus() {
        painter.rect_stroke(
            response.rect.shrink(1.0),
            egui::CornerRadius::same(6),
            egui::Stroke::new(1.5_f32, palette.accent),
            egui::StrokeKind::Middle,
        );
    }
    if selected {
        painter.rect_filled(
            egui::Rect::from_min_size(
                response.rect.left_top() + egui::vec2(0.0, 4.0),
                egui::vec2(3.0, response.rect.height() - 8.0),
            ),
            egui::CornerRadius::same(2),
            palette.accent,
        );
    }
    let icon_rect = egui::Rect::from_min_size(
        response.rect.left_top() + egui::vec2(10.0, 6.0),
        egui::vec2(24.0, 24.0),
    );
    ui::paint_tool_icon(
        &painter,
        icon_rect,
        descriptor.icon,
        if selected {
            palette.accent
        } else {
            palette.weak
        },
    );
    let text_color = if selected {
        palette.accent
    } else {
        palette.text
    };
    let galley = painter.layout_no_wrap(
        descriptor.name.to_owned(),
        egui::FontId::proportional(13.5),
        text_color,
    );
    painter.galley(
        response.rect.left_center() + egui::vec2(40.0, -galley.size().y * 0.5),
        galley,
        text_color,
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

/// 首页紧凑列表视图单行（专为海量工具设计的高密度扫描列表）。
fn home_tool_row(
    ui: &mut egui::Ui,
    descriptor: &ToolDescriptor,
    favorite: bool,
    striped: bool,
) -> (bool, bool) {
    let width = ui.available_width();
    let (response, painter) = ui.allocate_painter(egui::vec2(width, 42.0), egui::Sense::click());
    let palette = ui::palette_for_ui(ui);

    let fill = if response.hovered() {
        palette.hover
    } else if striped {
        ui.visuals().faint_bg_color
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(response.rect, egui::CornerRadius::same(4), fill);

    // 图标
    let icon_rect = egui::Rect::from_min_size(
        response.rect.left_top() + egui::vec2(8.0, 9.0),
        egui::vec2(24.0, 24.0),
    );
    ui::paint_tool_icon(&painter, icon_rect, descriptor.icon, palette.accent);

    // 名称
    painter.text(
        response.rect.left_center() + egui::vec2(40.0, 0.0),
        egui::Align2::LEFT_CENTER,
        descriptor.name,
        egui::FontId::proportional(14.0),
        palette.text,
    );

    // 分类
    let cat_rect = egui::Rect::from_center_size(
        response.rect.left_center() + egui::vec2(160.0, 0.0),
        egui::vec2(42.0, 18.0),
    );
    painter.rect_filled(cat_rect, egui::CornerRadius::same(4), palette.border_subtle);
    painter.text(
        cat_rect.center(),
        egui::Align2::CENTER_CENTER,
        descriptor.category.label(),
        egui::FontId::proportional(11.0),
        palette.weak,
    );

    // 描述缩略
    let desc_rect = egui::Rect::from_min_max(
        response.rect.left_center() + egui::vec2(200.0, -10.0),
        response.rect.right_center() - egui::vec2(120.0, -10.0),
    );
    painter.with_clip_rect(desc_rect).text(
        desc_rect.left_center(),
        egui::Align2::LEFT_CENTER,
        descriptor.description,
        egui::FontId::proportional(12.5),
        palette.weak,
    );

    // 收藏星标
    let fav_rect = egui::Rect::from_center_size(
        response.rect.right_center() - egui::vec2(90.0, 0.0),
        egui::vec2(24.0, 24.0),
    );
    let fav_response = ui.interact(
        fav_rect,
        ui.id().with(("list-fav", descriptor.id)),
        egui::Sense::click(),
    );
    let star_color = if favorite {
        palette.warning_text
    } else {
        palette.weak
    };
    ui::paint_app_icon(
        &painter,
        fav_rect.shrink(5.0),
        ui::AppIcon::Star,
        star_color,
    );

    // 进入按钮
    let btn_rect = egui::Rect::from_center_size(
        response.rect.right_center() - egui::vec2(44.0, 0.0),
        egui::vec2(56.0, 24.0),
    );
    let btn_fill = if response.hovered() {
        palette.primary_button
    } else {
        palette.secondary_button_bg
    };
    painter.rect(
        btn_rect,
        egui::CornerRadius::same(4),
        btn_fill,
        egui::Stroke::new(1.0_f32, palette.border_subtle),
        egui::StrokeKind::Middle,
    );
    painter.text(
        btn_rect.center(),
        egui::Align2::CENTER_CENTER,
        "进入",
        egui::FontId::proportional(12.0),
        if response.hovered() {
            Color32::WHITE
        } else {
            palette.text
        },
    );

    let open = response.clicked();
    let toggle_fav = fav_response.clicked();
    (open && !toggle_fav, toggle_fav)
}

/// 按当前注册顺序保留收藏项，清除旧版本遗留的无效或重复工具标识。
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

    use super::{actions, navigation};
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

        navigation::normalize_favorite_tools(&descriptors, &mut favorites);

        assert_eq!(favorites, ["file-lock", "process-inspector"]);
    }

    #[test]
    fn stale_finished_event_does_not_clear_or_route_latest_request() {
        let mut latest = HashMap::from([("port-inspector", 2)]);
        assert!(!actions::accept_task_event(
            &mut latest,
            "port-inspector",
            1,
            true
        ));
        assert_eq!(latest.get("port-inspector"), Some(&2));
        assert!(actions::accept_task_event(
            &mut latest,
            "port-inspector",
            2,
            true
        ));
        assert!(!latest.contains_key("port-inspector"));
    }
}
