//! Windows Toolbox 的 GUI 外壳。
//! 该模块拥有 HUD 顶栏、分类折叠手风琴侧边栏、主题管理、设置、后台任务协作与 Tool Invocation 路由。

use std::{
    collections::{HashMap, HashSet},
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

use state::{HomeViewMode, Notice, NoticeTone, Page};

/// Tool Host：统一协调 Tool Registry、UI、后台任务和系统集成。
pub struct ToolboxApp {
    registry: ToolRegistry,
    page: Page,
    search: String,
    home_view_mode: HomeViewMode,
    active_category_filter: Option<Option<ToolCategory>>, // None = All, Some(None) = Favorites, Some(Some(cat)) = Category
    collapsed_categories: HashSet<ToolCategory>,
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
            home_view_mode: HomeViewMode::Grid,
            active_category_filter: None,
            collapsed_categories: HashSet::new(),
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

    /// 绘制顶部科技 HUD 状态栏（应用 Logo、当前位置面包屑、主题切换与快捷操作）。
    fn render_top_hud(&mut self, context: &egui::Context) -> Vec<AppAction> {
        let mut actions = Vec::new();
        let palette = ui::theme_palette(if context.style().visuals.dark_mode {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });

        egui::TopBottomPanel::top("toolbox-top-hud")
            .frame(
                egui::Frame::new()
                    .fill(palette.panel)
                    .stroke(egui::Stroke::new(1.0_f32, palette.border_subtle))
                    .inner_margin(egui::Margin::symmetric(16, 8)),
            )
            .show(context, |ui| {
                ui.horizontal(|ui| {
                    // Logo 与品牌
                    let logo_bg = palette.accent.gamma_multiply(if ui.visuals().dark_mode {
                        0.20
                    } else {
                        0.12
                    });
                    ui::icon_badge(
                        ui,
                        crate::tools::ToolIcon::Process,
                        28.0,
                        palette.accent,
                        logo_bg,
                    );
                    ui.add_space(ui::SPACE_4);
                    ui.label(
                        RichText::new("TOOLBOX")
                            .strong()
                            .monospace()
                            .size(15.0)
                            .color(palette.text),
                    );
                    ui::badge(
                        ui,
                        concat!("v", env!("CARGO_PKG_VERSION")),
                        palette.accent_secondary,
                        palette.border_subtle,
                    );

                    // 状态微光指示灯
                    ui.add_space(ui::SPACE_8);
                    ui::status_pill(ui, "READY", palette.success_text);

                    ui.add_space(ui::SPACE_12);
                    ui.separator();
                    ui.add_space(ui::SPACE_12);

                    // 面包屑导航
                    match &self.page {
                        Page::Home => {
                            ui.label(RichText::new("控制中心").size(13.5).color(palette.weak));
                            ui.label(
                                RichText::new("/ 概览工作台")
                                    .size(13.5)
                                    .strong()
                                    .color(palette.accent),
                            );
                        }
                        Page::Tool(id) => {
                            if let Some(desc) = self.registry.get(id).map(|t| t.descriptor()) {
                                if ui
                                    .link(RichText::new("控制中心").size(13.0).color(palette.weak))
                                    .clicked()
                                {
                                    actions.push(AppAction::NavigateTo("home".into()));
                                }
                                ui.colored_label(palette.weak, "/");
                                ui.label(
                                    RichText::new(desc.category.label())
                                        .size(13.0)
                                        .color(palette.weak),
                                );
                                ui.colored_label(palette.weak, "/");
                                ui.label(
                                    RichText::new(desc.name)
                                        .size(13.5)
                                        .strong()
                                        .color(palette.accent),
                                );

                                // 收藏快捷按钮
                                let is_fav =
                                    self.settings.favorite_tools.iter().any(|fav| fav == id);
                                let fav_btn_text = if is_fav {
                                    "★ 已收藏"
                                } else {
                                    "☆ 收藏"
                                };
                                let star_color = if is_fav {
                                    palette.warning_text
                                } else {
                                    palette.weak
                                };
                                if ui
                                    .button(
                                        RichText::new(fav_btn_text).size(12.0).color(star_color),
                                    )
                                    .clicked()
                                {
                                    actions.push(AppAction::ToggleFavorite {
                                        tool_id: id.clone(),
                                    });
                                }
                            }
                        }
                        Page::Settings => {
                            if ui
                                .link(RichText::new("控制中心").size(13.0).color(palette.weak))
                                .clicked()
                            {
                                actions.push(AppAction::NavigateTo("home".into()));
                            }
                            ui.colored_label(palette.weak, "/");
                            ui.label(
                                RichText::new("系统设置")
                                    .size(13.5)
                                    .strong()
                                    .color(palette.accent),
                            );
                        }
                        Page::About => {
                            if ui
                                .link(RichText::new("控制中心").size(13.0).color(palette.weak))
                                .clicked()
                            {
                                actions.push(AppAction::NavigateTo("home".into()));
                            }
                            ui.colored_label(palette.weak, "/");
                            ui.label(
                                RichText::new("关于与架构")
                                    .size(13.5)
                                    .strong()
                                    .color(palette.accent),
                            );
                        }
                    }

                    // 右侧控制区
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // 主题切换快捷按钮
                        let next_theme = match self.settings.theme {
                            ThemePreference::Dark => ThemePreference::Light,
                            ThemePreference::Light => ThemePreference::System,
                            ThemePreference::System => ThemePreference::Dark,
                        };
                        let theme_icon = match self.settings.theme {
                            ThemePreference::Dark => "🌙 深色",
                            ThemePreference::Light => "☀ 浅色",
                            ThemePreference::System => "⚙ 跟随系统",
                        };
                        if ui::small_action_button(ui, theme_icon)
                            .on_hover_text("点击切换主题外观")
                            .clicked()
                        {
                            self.settings.theme = next_theme;
                            apply_theme(ui.ctx(), self.settings.theme);
                            self.persist_settings();
                        }
                    });
                });
            });

        actions
    }

    /// 绘制左侧多层级折叠手风琴导航栏（支持海量工具分类归纳与实时计数）。
    fn render_sidebar(&mut self, context: &egui::Context) -> Vec<AppAction> {
        let descriptors = if self.search.trim().is_empty() {
            Vec::new()
        } else {
            self.registry.search(&self.search)
        };
        let categories = self.registry.categories();
        let all_descriptors = self.registry.descriptors();
        let category_counts = navigation::category_counts(&all_descriptors);
        let mut actions = Vec::new();

        egui::SidePanel::left("toolbox-sidebar")
            .resizable(false)
            .default_width(260.0)
            .min_width(260.0)
            .show(context, |ui| {
                ui.add_space(ui::SPACE_12);

                // 科技搜索输入框
                let search_width = ui.available_width();
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [search_width, ui::CONTROL_HEIGHT],
                        ui::text_input(&mut self.search, "🔍 搜索全部工具 (中英/拼音)..."),
                    );
                });
                ui.add_space(ui::SPACE_12);

                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if !self.search.trim().is_empty() {
                            nav_section_label(ui, &format!("搜索结果 ({})", descriptors.len()));
                            ui.add_space(ui::SPACE_4);
                            if descriptors.is_empty() {
                                ui.label(
                                    RichText::new("未检索到匹配工具")
                                        .size(13.0)
                                        .color(ui.visuals().weak_text_color()),
                                );
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
                            // 主页工作台入口
                            let total_count = all_descriptors.len();
                            if nav_hero_button(
                                ui,
                                "工具工作台",
                                total_count,
                                self.page == Page::Home,
                            ) {
                                actions.push(AppAction::NavigateTo("home".into()));
                            }
                            ui.add_space(ui::SPACE_8);

                            // 我的收藏
                            let favorites = navigation::favorite_descriptors(
                                &all_descriptors,
                                &self.settings.favorite_tools,
                            );
                            if !favorites.is_empty() {
                                nav_section_label(ui, &format!("★ 我的收藏 ({})", favorites.len()));
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
                                ui.add_space(ui::SPACE_12);
                            }

                            // 分类可折叠手风琴 (Collapsible Category Accordions)
                            nav_section_label(ui, "工具大类");
                            ui.add_space(ui::SPACE_4);

                            for category in categories {
                                let count = category_counts.get(&category).copied().unwrap_or(0);
                                let is_collapsed = self.collapsed_categories.contains(&category);

                                // 折叠头按钮
                                if category_accordion_header(
                                    ui,
                                    category.label(),
                                    count,
                                    !is_collapsed,
                                ) {
                                    if is_collapsed {
                                        self.collapsed_categories.remove(&category);
                                    } else {
                                        self.collapsed_categories.insert(category);
                                    }
                                }

                                // 展开的工具列表
                                if !is_collapsed {
                                    for descriptor in self.registry.in_category(category) {
                                        if tool_nav_button(
                                            ui,
                                            &descriptor,
                                            self.page == Page::Tool(descriptor.id.into()),
                                        ) {
                                            actions
                                                .push(AppAction::NavigateTo(descriptor.id.into()));
                                        }
                                    }
                                }
                                ui.add_space(ui::SPACE_4);
                            }
                        }
                    });

                // 底部固定系统导航
                ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                    ui.add_space(ui::SPACE_8);
                    ui.separator();
                    ui.add_space(ui::SPACE_4);
                    if nav_text_button(ui, "ℹ 关于与架构", self.page == Page::About) {
                        actions.push(AppAction::NavigateTo("about".into()));
                    }
                    if nav_text_button(ui, "⚙ 系统设置", self.page == Page::Settings) {
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
                    .inner_margin(egui::Margin::symmetric(24, 16)),
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

    /// 绘制现代科技工作台（Toolbox Hub）：遥测看板、分类过滤胶囊、网格/列表视图切换、工具列表。
    fn render_home(&mut self, ui: &mut egui::Ui) -> Vec<AppAction> {
        let mut actions = Vec::new();
        let palette = ui::palette_for_ui(ui);
        let all_descriptors = self.registry.descriptors();
        let total_count = all_descriptors.len();
        let fav_count = self.settings.favorite_tools.len();
        let recent_count = self.settings.recent_tools.len();

        // 标头
        ui::page_heading(
            ui,
            "系统诊断工作台 (Toolbox Hub)",
            "原生高性能 Windows 系统诊断、网络路径探测与进程分析套件",
        );
        ui.add_space(ui::SPACE_16);

        // 顶部遥测指标看板 (Telemetry Tiles)
        let tile_width = ((ui.available_width() - ui::SPACE_12 * 3.0) / 4.0).max(140.0);
        ui.horizontal_wrapped(|ui| {
            ui::metric_tile(
                ui,
                tile_width,
                "收录工具总数",
                &total_count.to_string(),
                "个内建模块",
                palette.accent,
            );
            ui::metric_tile(
                ui,
                tile_width,
                "我的快捷收藏",
                &fav_count.to_string(),
                "项已星标",
                palette.warning_text,
            );
            ui::metric_tile(
                ui,
                tile_width,
                "最近运行记录",
                &recent_count.to_string(),
                "次启动",
                palette.accent_secondary,
            );
            ui::metric_tile(
                ui,
                tile_width,
                "诊断引擎状态",
                "READY",
                "原生运行中",
                palette.success_text,
            );
        });
        ui.add_space(ui::SPACE_16);

        // 分类筛选 Tab 栏与视图模式切换器
        ui.horizontal(|ui| {
            // 全部
            let is_all = self.active_category_filter.is_none();
            if category_filter_chip(ui, "全部工具", total_count, is_all, palette.accent) {
                self.active_category_filter = None;
            }

            // 收藏
            let is_fav_selected = self.active_category_filter == Some(None);
            if category_filter_chip(
                ui,
                "★ 收藏",
                fav_count,
                is_fav_selected,
                palette.warning_text,
            ) {
                self.active_category_filter = Some(None);
            }

            // 各个分类
            for category in self.registry.categories() {
                let count = all_descriptors
                    .iter()
                    .filter(|d| d.category == category)
                    .count();
                let is_selected = self.active_category_filter == Some(Some(category));
                if category_filter_chip(
                    ui,
                    category.label(),
                    count,
                    is_selected,
                    palette.accent_secondary,
                ) {
                    self.active_category_filter = Some(Some(category));
                }
            }

            // 右侧视图模式切换 (网格卡片 / 紧凑列表)
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui::small_action_button(
                    ui,
                    match self.home_view_mode {
                        HomeViewMode::Grid => "≡ 切换紧凑列表",
                        HomeViewMode::List => "田 切换网格卡片",
                    },
                )
                .on_hover_text("切换网格卡片或紧凑列表视图")
                .clicked()
                {
                    self.home_view_mode = match self.home_view_mode {
                        HomeViewMode::Grid => HomeViewMode::List,
                        HomeViewMode::List => HomeViewMode::Grid,
                    };
                }
            });
        });
        ui.add_space(ui::SPACE_12);

        // 过滤后的工具列表
        let filtered_descriptors: Vec<ToolDescriptor> = all_descriptors
            .into_iter()
            .filter(|desc| match self.active_category_filter {
                None => true,
                Some(None) => self.settings.favorite_tools.iter().any(|id| id == desc.id),
                Some(Some(cat)) => desc.category == cat,
            })
            .collect();

        if filtered_descriptors.is_empty() {
            ui.add_space(ui::SPACE_16);
            ui::state_card(
                ui,
                "当前分类下暂无工具",
                "请选择其他分类，或点击卡片右上角星标将常用工具加入收藏。",
                palette.weak,
            );
        } else {
            match self.home_view_mode {
                HomeViewMode::Grid => {
                    if layout::use_detail_split(ui.available_width()) {
                        ui.columns(2, |columns| {
                            for (index, descriptor) in filtered_descriptors.iter().enumerate() {
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
                        for descriptor in &filtered_descriptors {
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
                }
                HomeViewMode::List => {
                    ui::card(ui, |ui| {
                        for (index, descriptor) in filtered_descriptors.iter().enumerate() {
                            let is_fav = self
                                .settings
                                .favorite_tools
                                .iter()
                                .any(|id| id == descriptor.id);
                            let (open, fav) = home_tool_row(ui, descriptor, is_fav, index % 2 == 1);
                            if open {
                                actions.push(AppAction::NavigateTo(descriptor.id.into()));
                            }
                            if fav {
                                actions.push(AppAction::ToggleFavorite {
                                    tool_id: descriptor.id.into(),
                                });
                            }
                            if index + 1 < filtered_descriptors.len() {
                                ui.add_space(2.0);
                            }
                        }
                    });
                }
            }
        }

        // 最近使用工具栏
        ui.add_space(ui::SPACE_24);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("⏱ 最近运行")
                    .strong()
                    .size(14.5)
                    .color(palette.text),
            );
            if !self.settings.recent_tools.is_empty()
                && ui::small_action_button(ui, "清空记录").clicked()
            {
                actions.push(AppAction::ClearRecents);
            }
        });
        ui.add_space(ui::SPACE_8);

        if self.settings.recent_tools.is_empty() {
            ui.label(
                RichText::new("最近打开的工具会显示在这里，方便快速二次进入。")
                    .size(13.0)
                    .color(ui.visuals().weak_text_color()),
            );
        } else {
            let descriptors = self.registry.descriptors();
            ui.horizontal_wrapped(|ui| {
                for id in &self.settings.recent_tools {
                    if let Some(descriptor) = descriptors.iter().find(|d| d.id == id) {
                        let btn = egui::Button::new(
                            RichText::new(descriptor.name)
                                .size(12.5)
                                .color(palette.accent),
                        )
                        .fill(palette.secondary_button_bg)
                        .stroke(egui::Stroke::new(1.0_f32, palette.border_subtle))
                        .corner_radius(egui::CornerRadius::same(6));
                        if ui.add(btn).clicked() {
                            actions.push(AppAction::NavigateTo(descriptor.id.into()));
                        }
                    }
                }
            });
        }

        actions
    }

    fn render_settings(&mut self, ui: &mut egui::Ui) -> Vec<AppAction> {
        let mut actions = Vec::new();
        let palette = ui::palette_for_ui(ui);
        ui::page_heading(
            ui,
            "系统设置",
            "应用配置保存在当前 Windows 用户的本地目录。",
        );
        ui.add_space(ui::SPACE_20);

        ui::tech_card(ui, palette.accent, |ui| {
            ui.label(
                RichText::new("界面视觉风格与主题")
                    .strong()
                    .size(15.0)
                    .color(palette.text),
            );
            ui.add_space(2.0);
            ui.label(
                RichText::new("支持深空黑曜石赛博科技模式与明亮实验室工程模式。")
                    .size(13.0)
                    .color(palette.weak),
            );
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
                    ui.selectable_value(
                        &mut self.settings.theme,
                        ThemePreference::Light,
                        "极简浅色 (Light)",
                    );
                    ui.selectable_value(
                        &mut self.settings.theme,
                        ThemePreference::Dark,
                        "赛博黑曜石 (Dark)",
                    );
                });
            if before_theme != self.settings.theme {
                apply_theme(ui.ctx(), self.settings.theme);
                self.persist_settings();
            }
        });

        ui.add_space(ui::SPACE_16);
        ui::tech_card(ui, palette.accent_secondary, |ui| {
            ui.label(
                RichText::new("Windows 资源管理器右键集成")
                    .strong()
                    .size(15.0)
                    .color(palette.text),
            );
            ui.add_space(2.0);
            ui.label(
                RichText::new(
                    "在 Windows 文件右键菜单中注册“Windows Toolbox -> 查看文件占用”快捷入口。",
                )
                .size(13.0)
                .color(palette.weak),
            );
            ui.add_space(ui::SPACE_8);
            let registered = shell_context_menu::is_context_menu_registered();
            ui.horizontal(|ui| {
                let mut enabled = registered;
                if ui
                    .checkbox(&mut enabled, "启用 Windows Toolbox 右键菜单")
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
                if ui::primary_button(ui, "重新注册").clicked() {
                    actions.push(AppAction::ToggleContextMenu { enabled: true });
                }
                if registered && ui::secondary_button(ui, "移除菜单").clicked() {
                    actions.push(AppAction::ToggleContextMenu { enabled: false });
                }
            });
        });

        ui.add_space(ui::SPACE_16);
        ui::tech_card(ui, palette.weak, |ui| {
            ui.label(
                RichText::new("诊断日志与本地存储")
                    .strong()
                    .size(15.0)
                    .color(palette.text),
            );
            ui.add_space(2.0);
            ui.label(
                RichText::new("运行诊断与崩溃守护日志保存在本地配置目录，自动轮转最多保留三份。")
                    .size(13.0)
                    .color(palette.weak),
            );
            ui.add_space(ui::SPACE_8);
            let directory = crate::diagnostics::diagnostic_directory();
            if directory.is_some()
                && ui::secondary_button(ui, "打开诊断日志目录").clicked()
                && let Some(directory) = directory
            {
                actions.push(AppAction::OpenDirectory(directory));
            }
        });
        actions
    }

    fn render_about(&self, ui: &mut egui::Ui) -> Vec<AppAction> {
        let palette = ui::palette_for_ui(ui);
        ui::page_heading(
            ui,
            "关于 Windows Toolbox",
            concat!(
                "V",
                env!("CARGO_PKG_VERSION"),
                " · 原生高性能 Windows 系统诊断与工具集"
            ),
        );
        ui.add_space(ui::SPACE_20);
        ui::tech_card(ui, palette.accent, |ui| {
            ui.label("Windows Toolbox 基于 Rust 和 egui 原生开发，提供无冗余依赖、零运行时、低开销的系统级诊断与网络分析能力。");
            ui.add_space(ui::SPACE_16);
            ui.label(
                RichText::new("已收录内建诊断模块")
                    .strong()
                    .size(15.0)
                    .color(palette.text),
            );
            ui.add_space(ui::SPACE_8);
            ui.label("• 文件占用 · 通过 Windows Restart Manager 查找锁定文件的进程");
            ui.label("• 端口占用 · 通过 Windows IP Helper API 实时列出 TCP / UDP 监听与连接");
            ui.label("• 进程关系 · 通过 Toolhelp 快照解析完整父进程链与子进程详情");
            ui.label("• DNS 查询 · 原生解析 A / AAAA / CNAME / MX / TXT / NS / PTR 记录");
            ui.label("• Ping 测试 · 真实 ICMP 往返延迟与丢包率测试");
            ui.label("• TCP 端口测试 · 无侵入式 TCP 三次握手连通性与时延测试");
            ui.label("• MTR 路径诊断 · 原生 ICMP 逐跳观察路由、延迟和丢包");
            ui.label("• TCP 调试 · 客户端与多客户端服务端持续收发原始字节流");
            ui.label("• UDP 调试 · 单播、IPv4 广播、IPv4 组播与来源回复");
            ui.label("• 串口调试 · COM 口配置、持续收发与设备断开状态记录");
            ui.add_space(ui::SPACE_16);
            ui.label(
                RichText::new("提示：默认以当前用户权限运行；查询受保护的系统进程或核心服务可能需要以管理员身份运行。")
                    .size(12.5)
                    .color(palette.weak),
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

fn nav_section_label(ui: &mut egui::Ui, label: &str) {
    ui.label(
        RichText::new(label)
            .size(11.5)
            .strong()
            .color(ui.visuals().weak_text_color()),
    );
}

/// 侧边栏主工作台大按钮（带工具数量徽章）。
fn nav_hero_button(ui: &mut egui::Ui, label: &str, count: usize, selected: bool) -> bool {
    let available = ui.available_width();
    let (response, painter) =
        ui.allocate_painter(egui::vec2(available, 38.0), egui::Sense::click());
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
            egui::Stroke::new(1.0_f32, palette.accent.gamma_multiply(0.40))
        } else {
            egui::Stroke::NONE
        },
        egui::StrokeKind::Middle,
    );
    if selected {
        painter.rect_filled(
            egui::Rect::from_min_size(
                response.rect.left_top() + egui::vec2(0.0, 5.0),
                egui::vec2(3.5, response.rect.height() - 10.0),
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
    painter.text(
        response.rect.left_center() + egui::vec2(12.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(14.0),
        text_color,
    );

    // 右侧数量胶囊
    let count_text = count.to_string();
    let count_font = egui::FontId::monospace(11.0);
    let count_galley = painter.layout_no_wrap(count_text, count_font, palette.weak);
    let badge_rect = egui::Rect::from_center_size(
        response.rect.right_center() - egui::vec2(20.0, 0.0),
        egui::vec2(count_galley.size().x + 12.0, 18.0),
    );
    painter.rect_filled(
        badge_rect,
        egui::CornerRadius::same(9),
        palette.border_subtle,
    );
    painter.galley(
        badge_rect.left_center() + egui::vec2(6.0, -count_galley.size().y * 0.5),
        count_galley,
        palette.weak,
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

/// 侧边栏分类折叠手风琴头按钮。
fn category_accordion_header(ui: &mut egui::Ui, label: &str, count: usize, expanded: bool) -> bool {
    let available = ui.available_width();
    let (response, painter) =
        ui.allocate_painter(egui::vec2(available, 30.0), egui::Sense::click());
    let palette = ui::palette_for_ui(ui);
    let fill = if response.hovered() {
        palette.hover
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(response.rect, egui::CornerRadius::same(4), fill);

    // 矢量绘制现代折叠箭头（保证在任何系统上绝对不会显示为方块）
    let arrow_center = response.rect.left_center() + egui::vec2(12.0, 0.0);
    if expanded {
        // 向下箭头 ▾
        painter.line_segment(
            [
                arrow_center + egui::vec2(-4.0, -2.0),
                arrow_center + egui::vec2(0.0, 2.5),
            ],
            egui::Stroke::new(1.6_f32, palette.weak),
        );
        painter.line_segment(
            [
                arrow_center + egui::vec2(0.0, 2.5),
                arrow_center + egui::vec2(4.0, -2.0),
            ],
            egui::Stroke::new(1.6_f32, palette.weak),
        );
    } else {
        // 向右箭头 ▸
        painter.line_segment(
            [
                arrow_center + egui::vec2(-2.0, -4.0),
                arrow_center + egui::vec2(2.5, 0.0),
            ],
            egui::Stroke::new(1.6_f32, palette.weak),
        );
        painter.line_segment(
            [
                arrow_center + egui::vec2(2.5, 0.0),
                arrow_center + egui::vec2(-2.0, 4.0),
            ],
            egui::Stroke::new(1.6_f32, palette.weak),
        );
    }

    // 分类名
    painter.text(
        response.rect.left_center() + egui::vec2(24.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.0),
        palette.text,
    );

    // 计数徽章
    let count_text = count.to_string();
    let count_font = egui::FontId::monospace(10.5);
    let count_galley = painter.layout_no_wrap(count_text, count_font, palette.weak);
    let badge_rect = egui::Rect::from_center_size(
        response.rect.right_center() - egui::vec2(16.0, 0.0),
        egui::vec2(count_galley.size().x + 10.0, 16.0),
    );
    painter.rect_filled(
        badge_rect,
        egui::CornerRadius::same(8),
        palette.border_subtle,
    );
    painter.galley(
        badge_rect.left_center() + egui::vec2(5.0, -count_galley.size().y * 0.5),
        count_galley,
        palette.weak,
    );

    response.clicked()
}

fn nav_text_button(ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
    let available = ui.available_width();
    let (response, painter) =
        ui.allocate_painter(egui::vec2(available, 34.0), egui::Sense::click());
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
    painter.text(
        response.rect.left_center() + egui::vec2(12.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.5),
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
    let (response, painter) =
        ui.allocate_painter(egui::vec2(available, 36.0), egui::Sense::click());
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
    painter.text(
        response.rect.left_center() + egui::vec2(40.0, 0.0),
        egui::Align2::LEFT_CENTER,
        descriptor.name,
        egui::FontId::proportional(13.5),
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

/// 分类过滤胶囊按钮。
fn category_filter_chip(
    ui: &mut egui::Ui,
    label: &str,
    count: usize,
    selected: bool,
    active_color: Color32,
) -> bool {
    let text = format!("{label} ({count})");
    let font_id = egui::FontId::proportional(12.5);
    let palette = ui::palette_for_ui(ui);
    let text_color = if selected { active_color } else { palette.weak };
    let galley = ui.painter().layout_no_wrap(text, font_id, text_color);
    let size = Vec2::new(galley.size().x + 20.0, 28.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    let painter = ui.painter();

    let bg = if selected {
        active_color.gamma_multiply(if ui.visuals().dark_mode { 0.18 } else { 0.12 })
    } else if response.hovered() {
        palette.hover
    } else {
        palette.secondary_button_bg
    };

    let stroke = if selected {
        egui::Stroke::new(1.2_f32, active_color)
    } else {
        egui::Stroke::new(1.0_f32, palette.border_subtle)
    };

    painter.rect(
        rect,
        egui::CornerRadius::same(14),
        bg,
        stroke,
        egui::StrokeKind::Middle,
    );
    painter.galley(
        rect.left_center() + Vec2::new(10.0, -galley.size().y * 0.5),
        galley,
        text_color,
    );

    response.clicked()
}

/// 首页科技感工具卡片（网格视图）。
fn home_tool_card(ui: &mut egui::Ui, descriptor: &ToolDescriptor, favorite: bool) -> (bool, bool) {
    let width = ui.available_width().min(520.0);
    let (response, painter) = ui.allocate_painter(egui::vec2(width, 126.0), egui::Sense::click());
    let painter = painter.with_clip_rect(response.rect);
    let palette = ui::palette_for_ui(ui);

    let fill = if response.hovered() {
        palette.hover
    } else {
        palette.surface
    };
    let stroke = if response.hovered() {
        egui::Stroke::new(1.2_f32, palette.accent)
    } else {
        egui::Stroke::new(1.0_f32, palette.border)
    };
    painter.rect(
        response.rect,
        egui::CornerRadius::same(8),
        fill,
        stroke,
        egui::StrokeKind::Middle,
    );

    // 顶部微妙科技霓虹条
    let top_neon = egui::Rect::from_min_size(
        response.rect.left_top() + egui::vec2(8.0, 0.0),
        egui::vec2(response.rect.width() - 16.0, 2.0),
    );
    if response.hovered() {
        painter.rect_filled(top_neon, egui::CornerRadius::same(1), palette.accent);
    } else {
        painter.rect_filled(top_neon, egui::CornerRadius::same(1), palette.border_subtle);
    }

    if response.has_focus() {
        painter.rect_stroke(
            response.rect.shrink(1.0),
            egui::CornerRadius::same(8),
            egui::Stroke::new(2.0_f32, palette.accent),
            egui::StrokeKind::Middle,
        );
    }
    let content = response.rect.shrink(16.0);

    // 图标容器底色
    let icon_bg_rect = egui::Rect::from_min_size(content.left_top(), egui::vec2(40.0, 40.0));
    let icon_bg = palette
        .accent
        .gamma_multiply(if ui.visuals().dark_mode { 0.20 } else { 0.12 });
    painter.rect(
        icon_bg_rect,
        egui::CornerRadius::same(8),
        icon_bg,
        egui::Stroke::new(1.0_f32, palette.accent.gamma_multiply(0.30)),
        egui::StrokeKind::Middle,
    );
    ui::paint_tool_icon(&painter, icon_bg_rect, descriptor.icon, palette.accent);

    // 标题
    painter.text(
        content.left_top() + egui::vec2(52.0, 2.0),
        egui::Align2::LEFT_TOP,
        descriptor.name,
        egui::FontId::proportional(16.0),
        palette.text,
    );

    // 分类标签
    let category_tag = descriptor.category.label();
    let cat_bg = palette.border_subtle;
    let name_len = descriptor.name.chars().count() as f32;
    let cat_rect = egui::Rect::from_min_size(
        content.left_top() + egui::vec2(52.0 + name_len * 16.0 + 8.0, 3.0),
        egui::vec2(38.0, 18.0),
    );
    painter.rect_filled(cat_rect, egui::CornerRadius::same(4), cat_bg);
    painter.text(
        cat_rect.center(),
        egui::Align2::CENTER_CENTER,
        category_tag,
        egui::FontId::proportional(11.0),
        palette.weak,
    );

    // 收藏按钮
    let favorite_rect = egui::Rect::from_min_size(
        response.rect.right_top() - egui::vec2(36.0, -10.0),
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
        "加入收藏"
    });
    if favorite_response.has_focus() {
        painter.rect_stroke(
            favorite_rect.shrink(1.0),
            egui::CornerRadius::same(6),
            egui::Stroke::new(2.0_f32, palette.accent),
            egui::StrokeKind::Middle,
        );
    }
    let star_color = if favorite {
        palette.warning_text
    } else if favorite_response.hovered() {
        palette.accent
    } else {
        palette.weak
    };
    painter.text(
        favorite_rect.center(),
        egui::Align2::CENTER_CENTER,
        if favorite { "★" } else { "☆" },
        egui::FontId::proportional(18.0),
        star_color,
    );

    // 描述
    painter.text(
        content.left_top() + egui::vec2(52.0, 32.0),
        egui::Align2::LEFT_TOP,
        descriptor.description,
        egui::FontId::proportional(13.0),
        palette.weak,
    );

    // 底部操作链接
    let action_color = if response.hovered() {
        palette.accent
    } else {
        palette.weak
    };
    painter.text(
        content.right_bottom(),
        egui::Align2::RIGHT_BOTTOM,
        "进入工具 →",
        egui::FontId::proportional(12.5),
        action_color,
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
    painter.text(
        fav_rect.center(),
        egui::Align2::CENTER_CENTER,
        if favorite { "★" } else { "☆" },
        egui::FontId::proportional(16.0),
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
