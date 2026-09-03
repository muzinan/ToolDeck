//! Windows Toolbox 的 GUI 外壳。
//! 该模块拥有 HUD 顶栏、统一工具侧栏、固定深色主题、设置、后台任务协作与 Tool Invocation 路由。

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
        communication::{ActiveCommunicationSession, CommunicationDispatcher},
        invocation::ToolInvocation,
        worker::{RequestId, TaskDispatcher, TaskEvent, TaskEventEnvelope, TaskRequest},
    },
    model::{
        AppError, CommunicationEvent, CommunicationKind, CommunicationSessionState, ProcessSummary,
    },
    platform::windows::{
        open_directory, open_file_location, save_csv, save_log, shell_context_menu,
    },
    settings::{
        AppSettings, SafePresetFamily, SafePresetFormat, SafePresetTool, SafeToolPreset,
        SettingsStore,
    },
    tools::{
        ToolCategory, ToolDescriptor, ToolRegistry, ToolUiContext, UiReviewVariant, build_registry,
    },
    ui,
};

mod actions;
mod content;
mod layout;
mod navigation;
mod overlay;
mod state;

use state::{Notice, NoticeTone, Page};

/// 仅由 ui-review feature 构造的视觉审查参数；正式启动传入默认空值。
pub(crate) struct UiReviewOptions {
    pub(crate) page: Option<String>,
    pub(crate) width: Option<f32>,
    #[cfg(feature = "ui-review")]
    pub(crate) height: Option<f32>,
    #[cfg(feature = "ui-review")]
    pub(crate) screenshot_path: Option<PathBuf>,
}

/// 首页最近使用的内存记录，不写入设置文件，避免跨进程保存会话轨迹。
#[derive(Clone, Debug)]
struct RecentToolVisit {
    tool_id: String,
    session_summary: Option<String>,
    status: Option<String>,
    visited_at: String,
}

/// Tool Host：统一协调 Tool Registry、UI、后台任务和系统集成。
pub struct ToolboxApp {
    registry: ToolRegistry,
    page: Page,
    search: String,
    active_category: ToolCategory,
    compact_sidebar_open: bool,
    settings_store: SettingsStore,
    settings: AppSettings,
    recent_tool_visits: Vec<RecentToolVisit>,
    task_dispatcher: TaskDispatcher,
    task_receiver: Receiver<TaskEventEnvelope>,
    communication_dispatcher: CommunicationDispatcher,
    latest_requests: HashMap<&'static str, RequestId>,
    invocation_receiver: Receiver<ToolInvocation>,
    pending_invocations: Vec<ToolInvocation>,
    pending_termination: Option<ProcessSummary>,
    preset_editor: Option<usize>,
    notice: Option<Notice>,
    review_mode: bool,
    review_width: Option<f32>,
    review_variant: UiReviewVariant,
    #[cfg(feature = "ui-review")]
    review_target_height: Option<f32>,
    #[cfg(feature = "ui-review")]
    review_screenshot_path: Option<PathBuf>,
    #[cfg(feature = "ui-review")]
    review_frame_count: u8,
}

impl ToolboxApp {
    pub fn new(
        creation_context: &eframe::CreationContext<'_>,
        settings_store: SettingsStore,
        settings: AppSettings,
        settings_warning: Option<String>,
        invocation_receiver: Receiver<ToolInvocation>,
        initial_invocation: Option<ToolInvocation>,
        review: UiReviewOptions,
    ) -> Result<Self, AppError> {
        let mut settings = settings;
        let review_mode = review.page.is_some();
        let registry = build_registry();
        let favorites_before = settings.favorite_tools.clone();
        navigation::normalize_favorite_tools(&registry.descriptors(), &mut settings.favorite_tools);
        if !review_mode && settings.favorite_tools != favorites_before {
            let _ = settings_store.save(&settings);
        }
        ui::install_windows_fonts(&creation_context.egui_ctx);
        ui::configure_styles(&creation_context.egui_ctx);
        creation_context
            .egui_ctx
            .set_theme(egui::ThemePreference::Dark);
        let (task_dispatcher, task_receiver) = TaskDispatcher::new()?;
        let communication_dispatcher = CommunicationDispatcher::new();
        let (page, review_variant) = match review.page.as_deref() {
            Some("settings") => (Page::Settings, UiReviewVariant::Default),
            Some("about") => (Page::About, UiReviewVariant::Default),
            Some("home") | None => (Page::Home, UiReviewVariant::Default),
            Some("tcp-debug-server") => (
                Page::Tool("tcp-debug".to_owned()),
                UiReviewVariant::TcpServer,
            ),
            Some("udp-debug-special") => (
                Page::Tool("udp-debug".to_owned()),
                UiReviewVariant::UdpSpecial,
            ),
            Some(tool_id) if registry.get(tool_id).is_some() => {
                (Page::Tool(tool_id.to_owned()), UiReviewVariant::Default)
            }
            Some(_) => (Page::Home, UiReviewVariant::Default),
        };
        if review_mode && page == Page::Settings {
            settings.context_menu_enabled = true;
            if settings.presets.is_empty() {
                settings.presets = settings_review_presets();
            }
        }
        let active_category = match &page {
            Page::Tool(tool_id) => registry.get(tool_id),
            _ => None,
        }
        .map_or(ToolCategory::Diagnostic, |tool| tool.descriptor().category);
        Ok(Self {
            registry,
            page,
            search: String::new(),
            active_category,
            compact_sidebar_open: false,
            settings_store,
            settings,
            recent_tool_visits: Vec::new(),
            task_dispatcher,
            task_receiver,
            communication_dispatcher,
            latest_requests: HashMap::new(),
            invocation_receiver,
            pending_invocations: initial_invocation.into_iter().collect(),
            pending_termination: None,
            preset_editor: None,
            notice: (!review_mode)
                .then_some(settings_warning)
                .flatten()
                .map(|message| Notice {
                    message,
                    tone: NoticeTone::Danger,
                    expires_at: Instant::now() + Duration::from_secs(10),
                }),
            review_mode,
            review_width: review.width,
            review_variant,
            #[cfg(feature = "ui-review")]
            review_target_height: review.height,
            #[cfg(feature = "ui-review")]
            review_screenshot_path: review.screenshot_path,
            #[cfg(feature = "ui-review")]
            review_frame_count: 0,
        })
    }

    #[cfg(feature = "ui-review")]
    fn update_review_screenshot(&mut self, context: &egui::Context) {
        let screenshot = context.input(|input| {
            input.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(screenshot) = screenshot {
            if let Some(path) = self.review_screenshot_path.take() {
                let rgba = screenshot
                    .pixels
                    .iter()
                    .flat_map(|pixel| pixel.to_array())
                    .collect::<Vec<_>>();
                image::save_buffer(
                    path,
                    &rgba,
                    screenshot.size[0] as u32,
                    screenshot.size[1] as u32,
                    image::ColorType::Rgba8,
                )
                .expect("UI 审查截图写入失败");
                context.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
        }

        if self.review_screenshot_path.is_none() {
            return;
        }

        match self.review_frame_count {
            0 => context.set_pixels_per_point(1.0),
            1 => {
                if let (Some(width), Some(height)) = (self.review_width, self.review_target_height)
                {
                    context.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(
                        980.0, 640.0,
                    )));
                    context.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                        width, height,
                    )));
                }
            }
            7 => context
                .send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default())),
            _ => {}
        }
        self.review_frame_count = self.review_frame_count.saturating_add(1);
    }

    fn drain_background_messages(&mut self, context: &egui::Context) {
        while let Ok(envelope) = self.task_receiver.try_recv() {
            let is_finished = matches!(&envelope.event, TaskEvent::Finished(_));
            if !actions::accept_task_event(
                &mut self.latest_requests,
                envelope.request_key,
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
                TaskEvent::PingProgress(progress) => {
                    if let Some(tool) = self.registry.get_mut(envelope.target_tool_id) {
                        tool.handle_ping_progress(progress);
                    }
                }
                TaskEvent::TcpProbeProgress(progress) => {
                    if let Some(tool) = self.registry.get_mut(envelope.target_tool_id) {
                        tool.handle_tcp_probe_progress(progress);
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
            if let CommunicationEvent::Status { state, .. } = &envelope.event {
                self.update_recent_communication_status(envelope.kind.tool_id(), *state);
            }
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
        let (section, current) = match &self.page {
            Page::Home => (None, "首页".to_owned()),
            Page::Settings => (None, "设置".to_owned()),
            Page::About => (None, "关于".to_owned()),
            Page::Tool(tool_id) => self
                .registry
                .get(tool_id)
                .map(|tool| {
                    let descriptor = tool.descriptor();
                    (
                        Some(format!("{}工具", descriptor.category.label())),
                        descriptor.name.to_owned(),
                    )
                })
                .unwrap_or((None, "首页".to_owned())),
        };
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
                    if let Some(section) = section {
                        ui.label(RichText::new(section).size(13.0).color(palette.weak));
                        ui.label(RichText::new("›").size(15.0).color(palette.weak));
                    }
                    ui.label(RichText::new(current).size(13.5).color(palette.text));
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
                        let search_width = (ui.available_width() - 16.0).clamp(180.0, 320.0);
                        ui.add_sized(
                            [search_width, ui::CONTROL_HEIGHT],
                            ui::text_input(&mut self.search, "搜索工具"),
                        );
                    });
                });
            });

        actions
    }

    /// 绘制单列统一导航；窄窗口在同一列内切换图标态与展开态，避免双栏重复。
    fn render_sidebar(&mut self, context: &egui::Context) -> Vec<AppAction> {
        let descriptors = if self.search.trim().is_empty() {
            Vec::new()
        } else {
            self.registry.search(&self.search)
        };
        let categories = self.registry.categories();
        let nav_layout = layout::navigation_layout(
            self.review_width
                .unwrap_or_else(|| context.screen_rect().width()),
        );
        let expanded = nav_layout == layout::NavigationLayout::Wide
            || self.compact_sidebar_open
            || !self.search.trim().is_empty();
        let sidebar_width = if expanded {
            layout::UNIFIED_SIDEBAR_WIDTH
        } else {
            layout::COMPACT_SIDEBAR_WIDTH
        };
        let mut actions = Vec::new();

        egui::SidePanel::left("toolbox-unified-navigation")
            .resizable(false)
            .exact_width(sidebar_width)
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
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        ui::theme_palette(if context.style().visuals.dark_mode {
                            egui::Theme::Dark
                        } else {
                            egui::Theme::Light
                        })
                        .border_subtle,
                    ))
                    .outer_margin(egui::Margin::ZERO)
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(context, |ui| {
                if expanded {
                    let footer_height = layout::TOOL_ROW_HEIGHT + ui::SPACE_8;
                    let navigation_height = (ui.available_height() - footer_height).max(120.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(ui.available_width(), navigation_height),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            egui::ScrollArea::vertical()
                                .auto_shrink([true, false])
                                .show(ui, |ui| {
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    if nav_layout == layout::NavigationLayout::Compact {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(self.active_category.label())
                                                    .strong()
                                                    .size(15.0),
                                            );
                                            ui.with_layout(
                                                egui::Layout::right_to_left(egui::Align::Center),
                                                |ui| {
                                                    if ui::icon_button(
                                                        ui,
                                                        ui::AppIcon::Back,
                                                        "收起导航",
                                                        false,
                                                    )
                                                    .clicked()
                                                    {
                                                        self.compact_sidebar_open = false;
                                                    }
                                                },
                                            );
                                        });
                                        ui.add_space(ui::SPACE_8);
                                        ui.separator();
                                        ui.add_space(ui::SPACE_8);
                                    } else {
                                        if app_nav_button(
                                            ui,
                                            "首页",
                                            ui::AppIcon::Home,
                                            self.page == Page::Home,
                                            layout::HOME_ROW_HEIGHT,
                                        ) {
                                            actions.push(AppAction::NavigateTo("home".into()));
                                        }
                                        ui.allocate_ui_with_layout(
                                            egui::vec2(
                                                ui.available_width(),
                                                layout::CATEGORY_ROW_HEIGHT,
                                            ),
                                            egui::Layout::left_to_right(egui::Align::Center),
                                            |ui| {
                                                ui.add_space(ui::SPACE_12);
                                                category_segmented_control(
                                                    ui,
                                                    &categories,
                                                    &mut self.active_category,
                                                );
                                                ui.add_space(ui::SPACE_12);
                                            },
                                        );
                                        ui.separator();
                                    }

                                    egui::Frame::new()
                                        .inner_margin(egui::Margin::symmetric(16, 0))
                                        .show(ui, |ui| {
                                            if !self.search.trim().is_empty() {
                                                ui.label(
                                                    RichText::new("搜索结果").strong().size(13.0),
                                                );
                                                ui.add_space(ui::SPACE_8);
                                                if descriptors.is_empty() {
                                                    ui.label(
                                                        RichText::new("没有匹配的工具")
                                                            .size(12.5)
                                                            .color(ui.visuals().weak_text_color()),
                                                    );
                                                }
                                                for descriptor in &descriptors {
                                                    if tool_nav_button(
                                                        ui,
                                                        descriptor,
                                                        self.page
                                                            == Page::Tool(descriptor.id.into()),
                                                    ) {
                                                        actions.push(AppAction::NavigateTo(
                                                            descriptor.id.into(),
                                                        ));
                                                        if nav_layout
                                                            == layout::NavigationLayout::Compact
                                                        {
                                                            self.compact_sidebar_open = false;
                                                        }
                                                    }
                                                }
                                            } else {
                                                for descriptor in
                                                    self.registry.in_category(self.active_category)
                                                {
                                                    if tool_nav_button(
                                                        ui,
                                                        &descriptor,
                                                        self.page
                                                            == Page::Tool(descriptor.id.into()),
                                                    ) {
                                                        actions.push(AppAction::NavigateTo(
                                                            descriptor.id.into(),
                                                        ));
                                                        if nav_layout
                                                            == layout::NavigationLayout::Compact
                                                        {
                                                            self.compact_sidebar_open = false;
                                                        }
                                                    }
                                                }
                                            }
                                        });
                                });
                        },
                    );
                    ui.add_space(ui::SPACE_8);
                    if app_nav_button(
                        ui,
                        "设置",
                        ui::AppIcon::Settings,
                        self.page == Page::Settings,
                        layout::TOOL_ROW_HEIGHT,
                    ) {
                        actions.push(AppAction::NavigateTo("settings".into()));
                    }
                } else {
                    ui.vertical_centered(|ui| {
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
                        for category in categories.iter().copied() {
                            let selected = self.active_category == category
                                && matches!(self.page, Page::Tool(_));
                            if ui::icon_button_sized(
                                ui,
                                category_app_icon(category),
                                category.label(),
                                selected,
                                layout::CATEGORY_ROW_HEIGHT,
                            )
                            .clicked()
                            {
                                self.active_category = category;
                                self.compact_sidebar_open = true;
                            }
                        }
                    });
                    ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                        if ui::icon_button_sized(
                            ui,
                            ui::AppIcon::Settings,
                            "设置",
                            self.page == Page::Settings,
                            layout::CATEGORY_ROW_HEIGHT,
                        )
                        .clicked()
                        {
                            actions.push(AppAction::NavigateTo("settings".into()));
                        }
                    });
                }
            });
        actions
    }

    fn render_content(&mut self, context: &egui::Context) -> Vec<AppAction> {
        let page = self.page.clone();
        let palette = ui::theme_palette(if context.style().visuals.dark_mode {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(0, 1)),
            )
            .show(context, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(content::MAIN_SCROLL_ID)
                    .auto_shrink([false, false])
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        let content_w = self.review_width.map_or_else(
                            || layout::content_width(ui.available_width()),
                            |window_width| {
                                let navigation = layout::persistent_navigation_width(
                                    layout::navigation_layout(window_width),
                                );
                                layout::content_width(
                                    (window_width - navigation - ui::PAGE_PADDING * 2.0)
                                        .min(ui.available_width())
                                        .max(320.0),
                                )
                            },
                        );
                        let available_h = ui.available_height().max(500.0);
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.allocate_ui_with_layout(
                                egui::vec2(content_w, available_h),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    egui::Frame::new()
                                        .fill(palette.surface)
                                        .stroke(egui::Stroke::new(1.0_f32, palette.border))
                                        .corner_radius(egui::CornerRadius::same(6))
                                        .inner_margin(egui::Margin::same(
                                            ui::PAGE_PADDING as i8,
                                        ))
                                        .show(ui, |ui| {
                                            ui.set_min_height(
                                                (available_h - ui::PAGE_PADDING * 2.0).max(0.0),
                                            );
                                            match page {
                                                Page::Home => self.render_home(ui),
                                                Page::Tool(id) => self
                                                    .registry
                                                    .get_mut(&id)
                                                    .map(|tool| {
                                                        tool.ui(
                                                            ui,
                                                            ToolUiContext {
                                                                review_mode: self.review_mode,
                                                                review_variant: self.review_variant,
                                                            },
                                                        )
                                                    })
                                                    .unwrap_or_else(|| {
                                                        vec![AppAction::NavigateTo("home".into())]
                                                    }),
                                                Page::Settings => self.render_settings(ui),
                                                Page::About => self.render_about(ui),
                                            }
                                        })
                                        .inner
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

    /// 绘制首页的会话、收藏、最近使用与工具索引，结构与设计稿保持一致。
    fn render_home(&mut self, ui: &mut egui::Ui) -> Vec<AppAction> {
        let mut actions = Vec::new();
        let descriptors = self.registry.descriptors();
        let show_review_content = self.review_mode && matches!(&self.page, Page::Home);
        let active_sessions = if show_review_content {
            home_review_sessions()
        } else {
            self.communication_dispatcher.active_sessions()
        };
        let favorites = if show_review_content && self.settings.favorite_tools.is_empty() {
            home_review_favorites(&descriptors)
        } else {
            navigation::favorite_descriptors(&descriptors, &self.settings.favorite_tools)
        };
        let recent_visits = if show_review_content && self.recent_tool_visits.is_empty() {
            home_review_recent_visits()
        } else {
            self.recent_tool_visits.clone()
        };

        ui::page_heading(ui, "工作台", "网络诊断与通信调试");
        ui.add_space(ui::SPACE_16);

        if ui.available_width() < 760.0 {
            render_home_sessions(ui, &descriptors, &active_sessions, &mut actions);
            ui.add_space(ui::SPACE_12);
            render_home_favorites(ui, &favorites, &mut actions);
        } else {
            ui.columns(2, |columns| {
                render_home_sessions(&mut columns[0], &descriptors, &active_sessions, &mut actions);
                render_home_favorites(&mut columns[1], &favorites, &mut actions);
            });
        }

        ui.add_space(ui::SPACE_12);
        render_home_recent(
            ui,
            &descriptors,
            &recent_visits,
            &active_sessions,
            &mut actions,
        );

        ui.add_space(ui::SPACE_12);
        render_home_tool_index(
            ui,
            &self.registry.categories(),
            &descriptors,
            &self.settings.favorite_tools,
            &mut actions,
        );
        actions
    }

    fn render_settings(&mut self, ui: &mut egui::Ui) -> Vec<AppAction> {
        let mut actions = Vec::new();
        let palette = ui::palette_for_ui(ui);
        ui::page_heading(ui, "设置", "应用行为与非敏感参数预设");
        ui.add_space(ui::SPACE_16);
        ui.separator();
        ui.add_space(ui::SPACE_16);

        ui::section(ui, |ui| {
            ui.label(RichText::new("交互").strong().size(15.0));
            ui.add_space(ui::SPACE_12);
            let registered = self.review_mode || shell_context_menu::is_context_menu_registered();
            ui.horizontal(|ui| {
                let mut enabled = registered;
                if ui
                    .checkbox(&mut enabled, "启用右键菜单")
                    .changed()
                {
                    actions.push(AppAction::ToggleContextMenu { enabled });
                }
            });
        });

        let mut presets_changed = false;
        let mut remove_preset = None;
        let mut next_editor = self.preset_editor;
        ui::section(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("参数预设").strong().size(15.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.settings.presets.len() < 12
                        && ui::secondary_button(ui, "新建预设").clicked()
                    {
                        let mut preset = SafeToolPreset::default();
                        preset.updated_at = Some(settings_timestamp());
                        self.settings.presets.push(preset);
                        next_editor = self.settings.presets.len().checked_sub(1);
                        presets_changed = true;
                    }
                });
            });
            ui.add_space(ui::SPACE_4);
            ui.label(
                RichText::new("仅保存非敏感枚举与数值，不保存目标或会话数据。")
                    .color(palette.weak),
            );
            ui.add_space(ui::SPACE_12);
            ui::table_card(ui, |ui| {
                ui.set_min_width(ui.available_width());
                egui::Grid::new("settings-preset-table")
                    .striped(true)
                    .min_row_height(62.0)
                    .spacing([ui::SPACE_16, 0.0])
                    .show(ui, |ui| {
                        for header in ["名称", "适用工具", "参数摘要", "修改时间", "操作"] {
                            ui.label(RichText::new(header).strong().color(palette.weak));
                        }
                        ui.end_row();
                        for (index, preset) in self.settings.presets.iter().enumerate() {
                            ui.label(RichText::new(preset_display_name(preset)).strong());
                            ui.label(preset.tool.label());
                            ui.label(RichText::new(preset_summary(preset)).monospace());
                            ui.label(
                                RichText::new(
                                    preset.updated_at.as_deref().unwrap_or("未修改"),
                                )
                                .monospace(),
                            );
                            ui.horizontal(|ui| {
                                if ui::small_action_button(ui, "编辑").clicked() {
                                    next_editor = Some(index);
                                }
                                if ui::danger_button(ui, "删除").clicked() {
                                    remove_preset = Some(index);
                                }
                            });
                            ui.end_row();
                        }
                    });
            });
        });
        if let Some(index) = remove_preset {
            self.settings.presets.remove(index);
            presets_changed = true;
            next_editor = match next_editor {
                Some(current) if current == index => None,
                Some(current) if current > index => Some(current - 1),
                current => current,
            };
        }
        self.preset_editor = next_editor.filter(|index| *index < self.settings.presets.len());
        if let Some(index) = self.preset_editor {
            ui::section(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("编辑预设").strong().size(15.0));
                    ui.label(RichText::new(preset_display_name(&self.settings.presets[index])).color(palette.weak));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::secondary_button(ui, "完成").clicked() {
                            self.preset_editor = None;
                        }
                    });
                });
                ui.add_space(ui::SPACE_12);
                let preset = &mut self.settings.presets[index];
                let mut changed = false;
                ui.horizontal_wrapped(|ui| {
                    ui.label("工具");
                    egui::ComboBox::from_id_salt(("preset-tool", index))
                        .selected_text(preset.tool.label())
                        .show_ui(ui, |ui| {
                            for value in SafePresetTool::ALL {
                                changed |= ui.selectable_value(&mut preset.tool, value, value.label()).changed();
                            }
                        });
                    ui.label("地址族");
                    egui::ComboBox::from_id_salt(("preset-family", index))
                        .selected_text(preset.family.label())
                        .show_ui(ui, |ui| {
                            for value in SafePresetFamily::ALL {
                                changed |= ui.selectable_value(&mut preset.family, value, value.label()).changed();
                            }
                        });
                    ui.label("显示格式");
                    egui::ComboBox::from_id_salt(("preset-format", index))
                        .selected_text(preset.display_format.label())
                        .show_ui(ui, |ui| {
                            for value in SafePresetFormat::ALL {
                                changed |= ui.selectable_value(&mut preset.display_format, value, value.label()).changed();
                            }
                        });
                    changed |= ui.checkbox(&mut preset.append_crlf, "追加 CRLF").changed();
                });
                ui.add_space(ui::SPACE_8);
                ui.horizontal_wrapped(|ui| {
                    ui.label("超时(ms)");
                    changed |= ui.add(egui::DragValue::new(&mut preset.timeout_ms).range(100..=30_000)).changed();
                    ui.label("间隔(ms)");
                    changed |= ui.add(egui::DragValue::new(&mut preset.interval_ms).range(100..=3_600_000)).changed();
                    ui.label("次数");
                    changed |= ui.add(egui::DragValue::new(&mut preset.attempts).range(1..=100)).changed();
                    ui.label("波特率");
                    changed |= ui.add(egui::DragValue::new(&mut preset.serial_baud_rate).range(300..=4_000_000)).changed();
                });
                if changed {
                    preset.updated_at = Some(settings_timestamp());
                    presets_changed = true;
                }
            });
        }
        if presets_changed {
            self.persist_settings();
        }

        ui::section(ui, |ui| {
            ui.label(RichText::new("诊断").strong().size(15.0));
            ui.add_space(ui::SPACE_12);
            let directory = (!self.review_mode)
                .then(crate::diagnostics::diagnostic_directory)
                .flatten();
            let display_path = directory
                .as_ref()
                .map_or_else(|| "%LOCALAPPDATA%\\ToolDeck\\diagnostics".to_owned(), |path| path.display().to_string());
            ui.horizontal(|ui| {
                ui.label("诊断目录");
                let mut value = display_path.clone();
                ui.add_enabled(false, ui::text_input(&mut value, ""));
                if ui::secondary_button(ui, "打开诊断目录").clicked()
                    && let Some(path) = directory.clone()
                {
                    actions.push(AppAction::OpenDirectory(path));
                }
                if ui::secondary_button(ui, "复制路径").clicked() {
                    actions.push(AppAction::CopyText(display_path));
                }
            });
            ui.add_space(ui::SPACE_12);
            ui.separator();
            ui.add_space(ui::SPACE_12);
            ui.label(
                RichText::new("目标、路径、PID、端点、载荷、日志和系统结果不会持久化。")
                    .color(palette.weak),
            );
        });
        actions
    }

    fn render_about(&self, ui: &mut egui::Ui) -> Vec<AppAction> {
        let palette = ui::palette_for_ui(ui);
        let mut actions = Vec::new();
        let build_profile = if self.review_mode {
            "Release"
        } else if cfg!(debug_assertions) {
            "Debug"
        } else {
            "Release"
        };
        let license = if self.review_mode {
            "MIT License"
        } else {
            env!("CARGO_PKG_LICENSE")
        };
        let repository = env!("CARGO_PKG_REPOSITORY");
        let (build_time, commit) = if self.review_mode {
            ("2025-05-20 14:30:18", "a1b2c3d4")
        } else {
            (
                option_env!("TOOLDECK_BUILD_TIME").unwrap_or("本地构建"),
                option_env!("TOOLDECK_GIT_COMMIT").unwrap_or("未嵌入"),
            )
        };
        let build_info = format!(
            "构建时间： {build_time}\nCommit:    {commit}",
        );

        ui::page_heading(ui, "关于 ToolDeck", "面向 Windows 的网络诊断与通信调试工具箱");
        ui.add_space(48.0);

        ui::card(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                let (icon_rect, _) =
                    ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::hover());
                ui::paint_app_icon(
                    ui.painter(),
                    icon_rect.shrink(5.0),
                    ui::AppIcon::Developer,
                    palette.accent,
                );
                ui.add_space(ui::SPACE_8);
                about_product_value(ui, &format!("ToolDeck v{}", env!("CARGO_PKG_VERSION")), palette);
                ui.separator();
                about_product_value(ui, &format!("Windows {}", std::env::consts::ARCH), palette);
                ui.separator();
                about_product_value(ui, "Rust + egui", palette);
                ui.separator();
                about_product_value(ui, build_profile, palette);
            });
            ui.add_space(6.0);
        });
        ui.add_space(ui::SPACE_16);

        ui.label(RichText::new("能力清单").strong().size(15.0));
        ui.add_space(ui::SPACE_8);
        ui::card(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.set_min_height(266.0);
            ui.columns(3, |columns| {
                for (index, category) in self.registry.categories().iter().enumerate() {
                    about_tool_group(
                        &mut columns[index],
                        *category,
                        &self.registry.descriptors(),
                        &mut actions,
                    );
                }
            });
        });
        ui.add_space(26.0);

        ui.label(RichText::new("技术边界").strong().size(15.0));
        ui.add_space(ui::SPACE_8);
        ui::card(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.set_min_height(48.0);
            ui.horizontal_wrapped(|ui| {
                for (icon, label) in [
                    (ui::AppIcon::Network, "系统 DNS"),
                    (ui::AppIcon::Network, "纯 ICMP Ping"),
                    (ui::AppIcon::Developer, "原始 TCP（不含 TLS）"),
                    (ui::AppIcon::Network, "IPv4/IPv6"),
                    (ui::AppIcon::System, "Windows 串口"),
                ] {
                    about_technical_item(ui, icon, label, palette);
                }
            });
        });
        ui.add_space(ui::SPACE_16);
        ui.columns(2, |columns| {
            ui::card(&mut columns[0], |ui| {
                ui.set_min_height(119.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("开源许可").strong().size(15.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::icon_button(ui, ui::AppIcon::Copy, "复制开源信息", false).clicked() {
                            actions.push(AppAction::CopyText(format!("{license}\n{repository}")));
                        }
                    });
                });
                ui.add_space(ui::SPACE_8);
                ui.label(RichText::new(license).monospace().color(palette.text));
            });
            ui::card(&mut columns[1], |ui| {
                ui.set_min_height(119.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("构建信息").strong().size(15.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui::icon_button(ui, ui::AppIcon::Copy, "复制构建信息", false).clicked() {
                            actions.push(AppAction::CopyText(build_info.clone()));
                        }
                    });
                });
                ui.add_space(ui::SPACE_8);
                ui.label(RichText::new(build_info).monospace().color(palette.text));
            });
        });
        actions
    }

    fn handle_actions(&mut self, context: &egui::Context, actions: Vec<AppAction>) {
        if self.review_mode {
            for action in actions {
                if let AppAction::NavigateTo(target) = action {
                    self.navigate_to(&target);
                }
            }
            return;
        }
        for action in actions {
            match action {
                AppAction::NavigateTo(target) => self.navigate_to(&target),
                AppAction::InvokeTool(invocation) => self.apply_invocation(context, invocation),
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
                AppAction::ExportText { content, file_name } => {
                    match save_log(&content, &file_name) {
                        Ok(Some(path)) => self.set_notice(
                            format!("结果已导出到 {}", path.display()),
                            NoticeTone::Success,
                        ),
                        Ok(None) => {}
                        Err(error) => self.set_error_notice(error),
                    }
                }
                AppAction::StartCommunication(config) => {
                    let tool_id = config.kind().tool_id();
                    let summary = config.session_summary();
                    match self.communication_dispatcher.start(config) {
                        Ok(_) => self.update_recent_communication_summary(tool_id, summary),
                        Err(error) => self.set_error_notice(error),
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
                AppAction::StopDns => {
                    if let Some(request_id) = self.latest_requests.get("dns-lookup").copied() {
                        self.task_dispatcher.cancel(request_id);
                    }
                }
                AppAction::RunPing { host, config } => {
                    self.dispatch_task(TaskRequest::Ping { host, config })
                }
                AppAction::StopPing => {
                    if let Some(request_id) = self.latest_requests.get("ping").copied()
                        && self.task_dispatcher.cancel(request_id)
                    {
                        self.set_notice(
                            "已请求停止 Ping，将在当前回显结束后退出",
                            NoticeTone::Success,
                        );
                    }
                }
                AppAction::RunTcpProbe { host, port, config } => {
                    self.dispatch_task(TaskRequest::TcpProbe { host, port, config })
                }
                AppAction::StopTcpProbe => {
                    if let Some(request_id) = self.latest_requests.get("tcp-probe").copied()
                        && self.task_dispatcher.cancel(request_id)
                    {
                        self.set_notice(
                            "已请求停止 TCP 测试，将在当前连接结束后退出",
                            NoticeTone::Success,
                        );
                    }
                }
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
                AppAction::LoadPortProcessDetails { pid } => {
                    self.dispatch_task(TaskRequest::PortProcessDetails { pid })
                }
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
                    self.recent_tool_visits.clear();
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
        let request_key = request.request_key();
        let marks_tool_busy = request.marks_tool_busy();
        match self.task_dispatcher.dispatch(request) {
            Ok(request_id) => {
                self.latest_requests.insert(request_key, request_id);
                if marks_tool_busy && let Some(tool) = self.registry.get_mut(tool_id) {
                    tool.set_busy(true);
                }
            }
            Err(error) => {
                let still_pending = self.latest_requests.contains_key(request_key);
                if marks_tool_busy && let Some(tool) = self.registry.get_mut(tool_id) {
                    tool.set_busy(still_pending);
                }
                self.set_error_notice(error);
            }
        }
    }

    fn record_recent_tool(&mut self, tool_id: &str) {
        let active_session = self
            .communication_dispatcher
            .active_sessions()
            .into_iter()
            .find(|session| session.kind.tool_id() == tool_id);
        self.recent_tool_visits
            .retain(|existing| existing.tool_id != tool_id);
        self.recent_tool_visits.insert(
            0,
            RecentToolVisit {
                tool_id: tool_id.to_owned(),
                session_summary: active_session.as_ref().map(|session| session.summary.clone()),
                status: active_session
                    .as_ref()
                    .map(|session| session.state.label().to_owned()),
                visited_at: crate::platform::windows::local_date_time_millis(),
            },
        );
        self.recent_tool_visits.truncate(8);
    }

    fn update_recent_communication_summary(&mut self, tool_id: &str, summary: String) {
        if let Some(visit) = self
            .recent_tool_visits
            .iter_mut()
            .find(|visit| visit.tool_id == tool_id)
        {
            visit.session_summary = Some(summary);
            visit.status = Some(CommunicationSessionState::Starting.label().into());
        }
    }

    fn update_recent_communication_status(
        &mut self,
        tool_id: &str,
        state: CommunicationSessionState,
    ) {
        if let Some(visit) = self
            .recent_tool_visits
            .iter_mut()
            .find(|visit| visit.tool_id == tool_id)
        {
            visit.status = Some(state.label().into());
        }
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
        if self.review_mode {
            return;
        }
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
        #[cfg(feature = "ui-review")]
        self.update_review_screenshot(context);
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

fn home_review_sessions() -> Vec<ActiveCommunicationSession> {
    vec![
        ActiveCommunicationSession {
            kind: CommunicationKind::Tcp,
            summary: "192.0.2.10:8080".into(),
            state: CommunicationSessionState::Connected,
            status_detail: "已建立 TCP 连接".into(),
        },
        ActiveCommunicationSession {
            kind: CommunicationKind::Serial,
            summary: "COM3，115200，8N1".into(),
            state: CommunicationSessionState::Connected,
            status_detail: "串口已打开".into(),
        },
    ]
}

fn home_review_favorites(descriptors: &[ToolDescriptor]) -> Vec<ToolDescriptor> {
    ["dns-lookup", "ping", "tcp-debug"]
        .into_iter()
        .filter_map(|id| descriptors.iter().find(|descriptor| descriptor.id == id).cloned())
        .collect()
}

fn home_review_recent_visits() -> Vec<RecentToolVisit> {
    [
        (
            "tcp-debug",
            "192.0.2.10:8080",
            "已连接",
            "2025-05-20 14:32:18",
        ),
        (
            "serial-debug",
            "COM3，115200，8N1",
            "已打开",
            "2025-05-20 13:47:02",
        ),
        (
            "dns-lookup",
            "www.example.com",
            "已完成",
            "2025-05-20 11:25:33",
        ),
        (
            "ping",
            "198.51.100.8",
            "已完成",
            "2025-05-20 10:12:09",
        ),
        (
            "tcp-probe",
            "192.0.2.10:22",
            "已完成",
            "2025-05-19 17:58:44",
        ),
    ]
    .into_iter()
    .map(|(tool_id, session_summary, status, visited_at)| RecentToolVisit {
        tool_id: tool_id.into(),
        session_summary: Some(session_summary.into()),
        status: Some(status.into()),
        visited_at: visited_at.into(),
    })
    .collect()
}

fn render_home_sessions(
    ui: &mut egui::Ui,
    descriptors: &[ToolDescriptor],
    sessions: &[ActiveCommunicationSession],
    actions: &mut Vec<AppAction>,
) {
    ui::card(ui, |ui| {
        home_panel_header(ui, "运行中的会话");
        if sessions.is_empty() {
            ui.add_space(ui::SPACE_12);
            ui.label(
                RichText::new("暂无运行会话")
                    .size(12.5)
                    .color(ui.visuals().weak_text_color()),
            );
            ui.add_space(ui::SPACE_12);
            return;
        }
        for (index, session) in sessions.iter().enumerate() {
            if index > 0 {
                ui.separator();
            }
            if let Some(descriptor) = descriptors
                .iter()
                .find(|descriptor| descriptor.id == session.kind.tool_id())
            {
                home_active_session_row(ui, descriptor, session, actions);
            }
        }
    });
}

fn render_home_favorites(
    ui: &mut egui::Ui,
    favorites: &[ToolDescriptor],
    actions: &mut Vec<AppAction>,
) {
    ui::card(ui, |ui| {
        home_panel_header(ui, "收藏");
        if favorites.is_empty() {
            ui.add_space(ui::SPACE_12);
            ui.label(
                RichText::new("暂无收藏")
                    .size(12.5)
                    .color(ui.visuals().weak_text_color()),
            );
            ui.add_space(ui::SPACE_12);
            return;
        }
        for (row_index, row) in favorites.chunks(3).enumerate() {
            if row_index > 0 {
                ui.separator();
            }
            ui.columns(3, |columns| {
                for (index, column) in columns.iter_mut().enumerate() {
                    if let Some(descriptor) = row.get(index) {
                        let (open, remove) = home_favorite_item(column, descriptor);
                        if open {
                            actions.push(AppAction::NavigateTo(descriptor.id.into()));
                        }
                        if remove {
                            actions.push(AppAction::ToggleFavorite {
                                tool_id: descriptor.id.into(),
                            });
                        }
                    }
                }
            });
        }
    });
}

fn home_panel_header(ui: &mut egui::Ui, title: &str) {
    ui.label(RichText::new(title).strong().size(15.5));
    ui.add_space(ui::SPACE_8);
    ui.separator();
    ui.add_space(ui::SPACE_4);
}

fn home_active_session_row(
    ui: &mut egui::Ui,
    descriptor: &ToolDescriptor,
    session: &ActiveCommunicationSession,
    actions: &mut Vec<AppAction>,
) {
    let palette = ui::palette_for_ui(ui);
    ui.add_space(ui::SPACE_4);
    ui.horizontal(|ui| {
        ui::tool_icon(ui, descriptor.icon, 34.0, palette.accent);
        ui.add_space(ui::SPACE_4);
        ui.vertical(|ui| {
            ui.label(RichText::new(descriptor.name).strong().size(14.0));
            ui.label(
                RichText::new(&session.summary)
                    .monospace()
                    .size(12.0)
                    .color(palette.weak),
            );
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui::secondary_button_sized(ui, "继续", [62.0, ui::CONTROL_HEIGHT]).clicked() {
                actions.push(AppAction::NavigateTo(descriptor.id.into()));
            }
            ui.add_space(ui::SPACE_8);
            let status = home_status(
                ui,
                session.state.label(),
                home_session_color(session.state, palette),
            );
            status.on_hover_text(&session.status_detail);
        });
    });
    ui.add_space(ui::SPACE_4);
}

fn home_favorite_item(ui: &mut egui::Ui, descriptor: &ToolDescriptor) -> (bool, bool) {
    let width = ui.available_width();
    let (response, painter) = ui.allocate_painter(egui::vec2(width, 80.0), egui::Sense::click());
    let palette = ui::palette_for_ui(ui);
    let fill = if response.hovered() {
        palette.hover
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(response.rect, egui::CornerRadius::same(4), fill);
    let icon_rect = egui::Rect::from_min_size(
        response.rect.left_top() + egui::vec2(10.0, 14.0),
        egui::vec2(28.0, 28.0),
    );
    ui::paint_tool_icon(&painter, icon_rect, descriptor.icon, palette.accent);
    let text_rect = egui::Rect::from_min_max(
        response.rect.left_top() + egui::vec2(48.0, 10.0),
        response.rect.right_bottom() - egui::vec2(8.0, 8.0),
    );
    let clipped = painter.with_clip_rect(text_rect);
    clipped.text(
        text_rect.left_top(),
        egui::Align2::LEFT_TOP,
        descriptor.name,
        egui::FontId::proportional(13.5),
        palette.text,
    );
    clipped.text(
        text_rect.left_top() + egui::vec2(0.0, 22.0),
        egui::Align2::LEFT_TOP,
        descriptor.description,
        egui::FontId::proportional(11.5),
        palette.weak,
    );
    let mut remove = false;
    response.context_menu(|ui| {
        if ui.button("取消收藏").clicked() {
            remove = true;
        }
    });
    (response.clicked(), remove)
}

fn home_status(ui: &mut egui::Ui, label: &str, color: Color32) -> egui::Response {
    ui.horizontal(|ui| {
        let (response, painter) = ui.allocate_painter(egui::vec2(12.0, 18.0), egui::Sense::hover());
        painter.circle_filled(response.rect.center(), 5.0, color.gamma_multiply(0.28));
        painter.circle_filled(response.rect.center(), 3.5, color);
        ui.label(RichText::new(label).size(12.5).color(color));
    })
    .response
}

fn home_session_color(state: CommunicationSessionState, palette: ui::ThemePalette) -> Color32 {
    match state {
        CommunicationSessionState::Connected | CommunicationSessionState::Listening => {
            palette.success_text
        }
        CommunicationSessionState::Starting => palette.accent,
        CommunicationSessionState::Failed => palette.danger_text,
        CommunicationSessionState::Stopped => palette.weak,
    }
}

fn settings_timestamp() -> String {
    crate::platform::windows::local_date_time_millis()
        .chars()
        .take(19)
        .collect()
}

/// 返回视觉审查页的固定非敏感预设，不会写入用户本地设置。
fn settings_review_presets() -> Vec<SafeToolPreset> {
    vec![
        SafeToolPreset {
            updated_at: Some("2025-05-20 14:32:18".into()),
            tool: SafePresetTool::Ping,
            family: SafePresetFamily::V4,
            timeout_ms: 1_000,
            interval_ms: 1_000,
            attempts: 4,
            display_format: SafePresetFormat::Text,
            append_crlf: false,
            serial_baud_rate: 115_200,
        },
        SafeToolPreset {
            updated_at: Some("2025-05-19 17:58:44".into()),
            tool: SafePresetTool::TcpProbe,
            family: SafePresetFamily::V4,
            timeout_ms: 3_000,
            interval_ms: 1_000,
            attempts: 3,
            display_format: SafePresetFormat::Text,
            append_crlf: false,
            serial_baud_rate: 115_200,
        },
        SafeToolPreset {
            updated_at: Some("2025-05-20 13:47:02".into()),
            tool: SafePresetTool::SerialDebug,
            family: SafePresetFamily::Auto,
            timeout_ms: 1_000,
            interval_ms: 1_000,
            attempts: 1,
            display_format: SafePresetFormat::Text,
            append_crlf: false,
            serial_baud_rate: 115_200,
        },
    ]
}

fn preset_display_name(preset: &SafeToolPreset) -> String {
    match preset.tool {
        SafePresetTool::Ping => "快速 Ping".into(),
        SafePresetTool::TcpProbe => "TCP 重试".into(),
        SafePresetTool::Mtr => "MTR 路径诊断".into(),
        SafePresetTool::TcpDebug => "TCP 调试参数".into(),
        SafePresetTool::UdpDebug => "UDP 调试参数".into(),
        SafePresetTool::SerialDebug => format!("串口 {} 8N1", preset.serial_baud_rate),
    }
}

fn preset_summary(preset: &SafeToolPreset) -> String {
    if preset.tool == SafePresetTool::SerialDebug {
        return format!(
            "波特率={}, 数据位=8, 校验=无, 停止位=1, 流控=无",
            preset.serial_baud_rate
        );
    }
    format!(
        "{}, 超时={}ms, 间隔={}ms, 次数={}",
        preset.family.label(), preset.timeout_ms, preset.interval_ms, preset.attempts
    )
}

fn about_product_value(ui: &mut egui::Ui, value: &str, palette: ui::ThemePalette) {
    ui.label(
        RichText::new(value)
            .monospace()
            .size(13.0)
            .color(palette.text),
    );
}

fn about_tool_group(
    ui: &mut egui::Ui,
    category: ToolCategory,
    descriptors: &[ToolDescriptor],
    actions: &mut Vec<AppAction>,
) {
    let palette = ui::palette_for_ui(ui);
    ui.horizontal(|ui| {
        let (dot_rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 18.0), egui::Sense::hover());
        ui.painter()
            .circle_filled(dot_rect.center(), 4.0, palette.accent);
        ui.label(
            RichText::new(category.label())
                .strong()
                .size(14.0)
                .color(palette.text),
        );
    });
    ui.add_space(ui::SPACE_8);
    for descriptor in descriptors
        .iter()
        .filter(|descriptor| descriptor.category == category)
    {
        ui.horizontal(|ui| {
            ui::tool_icon(ui, descriptor.icon, 24.0, palette.weak);
            let response = ui.add(
                egui::Label::new(RichText::new(descriptor.name).size(13.5).color(palette.text))
                    .sense(egui::Sense::click()),
            );
            response.clone().on_hover_text(descriptor.description);
            if response.clicked() {
                actions.push(AppAction::NavigateTo(descriptor.id.into()));
            }
        });
        ui.add_space(ui::SPACE_4);
    }
}

fn about_technical_item(
    ui: &mut egui::Ui,
    icon: ui::AppIcon,
    label: &str,
    palette: ui::ThemePalette,
) {
    ui.horizontal(|ui| {
        let (icon_rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
        ui::paint_app_icon(ui.painter(), icon_rect.shrink(2.0), icon, palette.text);
        ui.label(RichText::new(label).size(13.0).color(palette.text));
    });
    ui.add_space(ui::SPACE_16);
}

fn category_app_icon(category: ToolCategory) -> ui::AppIcon {
    match category {
        ToolCategory::Diagnostic => ui::AppIcon::System,
        ToolCategory::Network => ui::AppIcon::Network,
        ToolCategory::Communication => ui::AppIcon::Developer,
    }
}

fn app_nav_button(
    ui: &mut egui::Ui,
    label: &str,
    icon: ui::AppIcon,
    selected: bool,
    height: f32,
) -> bool {
    let available = ui.available_width();
    let (response, painter) = ui.allocate_painter(
        egui::vec2(available, height),
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
    let icon_rect = egui::Rect::from_min_size(
        response.rect.left_top() + egui::vec2(12.0, 10.0),
        egui::vec2(26.0, 26.0),
    );
    ui::paint_app_icon(
        &painter,
        icon_rect,
        icon,
        if selected {
            palette.accent
        } else {
            palette.weak
        },
    );
    let galley = painter.layout_no_wrap(
        label.to_owned(),
        egui::FontId::proportional(16.0),
        text_color,
    );
    painter.galley(
        response.rect.left_center() + egui::vec2(48.0, -galley.size().y * 0.5),
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

/// 绘制三个固定分类标签；宽窗口只显示当前分类的工具，避免旧双层导航重复占用空间。
fn category_segmented_control(
    ui: &mut egui::Ui,
    categories: &[ToolCategory],
    active_category: &mut ToolCategory,
) {
    let palette = ui::palette_for_ui(ui);
    let count = categories.len().max(1) as f32;
    let item_width = (ui.available_width() / count).floor().max(52.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for category in categories {
            let selected = *active_category == *category;
            let response = ui.add_sized(
                [item_width, layout::CATEGORY_ROW_HEIGHT],
                egui::Button::selectable(
                    selected,
                    RichText::new(category.label()).size(16.0),
                ),
            );
            if response.clicked() {
                *active_category = *category;
            }
            if response.has_focus() {
                ui.painter().rect_stroke(
                    response.rect.shrink(1.0),
                    egui::CornerRadius::same(4),
                    egui::Stroke::new(1.5_f32, palette.accent),
                    egui::StrokeKind::Middle,
                );
            }
        }
    });
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
    let text_color = if selected {
        palette.accent
    } else {
        palette.text
    };
    let icon_rect = egui::Rect::from_min_size(
        response.rect.left_top() + egui::vec2(12.0, 11.0),
        egui::vec2(26.0, 26.0),
    );
    ui::paint_tool_icon(
        &painter,
        icon_rect,
        descriptor.icon,
        if selected { palette.accent } else { palette.weak },
    );
    let galley = painter.layout_no_wrap(
        descriptor.name.to_owned(),
        egui::FontId::proportional(16.0),
        text_color,
    );
    painter.galley(
        response.rect.left_center() + egui::vec2(48.0, -galley.size().y * 0.5),
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

fn render_home_recent(
    ui: &mut egui::Ui,
    descriptors: &[ToolDescriptor],
    visits: &[RecentToolVisit],
    active_sessions: &[ActiveCommunicationSession],
    actions: &mut Vec<AppAction>,
) {
    ui::card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("最近使用").strong().size(15.5));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !visits.is_empty()
                    && ui::icon_button(ui, ui::AppIcon::Clear, "清空最近使用", false).clicked()
                {
                    actions.push(AppAction::ClearRecents);
                }
            });
        });
        ui.add_space(ui::SPACE_8);
        ui.separator();
        if visits.is_empty() {
            ui.add_space(ui::SPACE_12);
            ui.label(
                RichText::new("当前会话尚未使用工具")
                    .size(12.5)
                    .color(ui.visuals().weak_text_color()),
            );
            ui.add_space(ui::SPACE_12);
            return;
        }
        home_recent_header(ui);
        for (index, visit) in visits.iter().enumerate() {
            let Some(descriptor) = descriptors
                .iter()
                .find(|descriptor| descriptor.id == visit.tool_id)
            else {
                continue;
            };
            if home_recent_row(ui, index, descriptor, visit, active_sessions) {
                actions.push(AppAction::NavigateTo(descriptor.id.into()));
            }
        }
    });
}

fn home_recent_header(ui: &mut egui::Ui) {
    let widths = home_recent_columns(ui.available_width());
    let total_width: f32 = widths.iter().sum();
    let (response, painter) = ui.allocate_painter(
        egui::vec2(total_width, 34.0),
        egui::Sense::hover(),
    );
    let palette = ui::palette_for_ui(ui);
    painter.rect_filled(
        response.rect,
        egui::CornerRadius::same(4),
        ui.visuals().faint_bg_color,
    );
    for (index, label) in ["工具名称", "会话（仅会话信息）", "状态", "最后使用时间"]
        .iter()
        .enumerate()
    {
        let cell = home_table_cell(response.rect, &widths, index);
        paint_home_table_text(&painter, cell, label, egui::FontId::proportional(12.0), palette.weak);
        if index > 0 {
            painter.line_segment(
                [
                    egui::pos2(cell.left(), cell.top()),
                    egui::pos2(cell.left(), cell.bottom()),
                ],
                egui::Stroke::new(1.0_f32, palette.border_subtle),
            );
        }
    }
}

fn home_recent_row(
    ui: &mut egui::Ui,
    index: usize,
    descriptor: &ToolDescriptor,
    visit: &RecentToolVisit,
    active_sessions: &[ActiveCommunicationSession],
) -> bool {
    let widths = home_recent_columns(ui.available_width());
    let total_width: f32 = widths.iter().sum();
    let (response, painter) = ui.allocate_painter(
        egui::vec2(total_width, 42.0),
        egui::Sense::click(),
    );
    let palette = ui::palette_for_ui(ui);
    let active_session = active_sessions
        .iter()
        .find(|session| session.kind.tool_id() == descriptor.id);
    let (summary, status, status_color) = if let Some(session) = active_session {
        (
            session.summary.clone(),
            session.state.label().to_owned(),
            home_session_color(session.state, palette),
        )
    } else {
        let status = visit.status.clone().unwrap_or_else(|| "已打开".into());
        let color = match status.as_str() {
            "失败" => palette.danger_text,
            "启动中" => palette.accent,
            _ => palette.success_text,
        };
        (
            visit
                .session_summary
                .clone()
                .unwrap_or_else(|| "本次会话".into()),
            status,
            color,
        )
    };
    let fill = if response.hovered() {
        palette.hover
    } else if index % 2 == 1 {
        ui.visuals().faint_bg_color.gamma_multiply(0.55)
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(response.rect, egui::CornerRadius::same(2), fill);
    painter.line_segment(
        [
            egui::pos2(response.rect.left(), response.rect.bottom()),
            egui::pos2(response.rect.right(), response.rect.bottom()),
        ],
        egui::Stroke::new(1.0_f32, palette.border_subtle),
    );
    for cell_index in 1..widths.len() {
        let cell = home_table_cell(response.rect, &widths, cell_index);
        painter.line_segment(
            [
                egui::pos2(cell.left(), cell.top()),
                egui::pos2(cell.left(), cell.bottom()),
            ],
            egui::Stroke::new(1.0_f32, palette.border_subtle),
        );
    }

    let tool_cell = home_table_cell(response.rect, &widths, 0);
    let icon_rect = egui::Rect::from_min_size(
        tool_cell.left_center() + egui::vec2(8.0, -11.0),
        egui::vec2(22.0, 22.0),
    );
    ui::paint_tool_icon(&painter, icon_rect, descriptor.icon, palette.accent);
    paint_home_table_text(
        &painter,
        tool_cell.translate(egui::vec2(34.0, 0.0)),
        descriptor.name,
        egui::FontId::proportional(13.0),
        palette.text,
    );
    paint_home_table_text(
        &painter,
        home_table_cell(response.rect, &widths, 1),
        &summary,
        egui::FontId::monospace(12.0),
        palette.text,
    );
    let status_cell = home_table_cell(response.rect, &widths, 2);
    let dot_center = status_cell.left_center() + egui::vec2(12.0, 0.0);
    painter.circle_filled(dot_center, 3.5, status_color);
    paint_home_table_text(
        &painter,
        status_cell.translate(egui::vec2(20.0, 0.0)),
        &status,
        egui::FontId::proportional(12.5),
        status_color,
    );
    paint_home_table_text(
        &painter,
        home_table_cell(response.rect, &widths, 3),
        &visit.visited_at,
        egui::FontId::monospace(12.0),
        palette.text,
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

fn home_recent_columns(width: f32) -> [f32; 4] {
    let name = (width * 0.24).clamp(150.0, 220.0);
    let status = (width * 0.17).clamp(118.0, 160.0);
    let time = (width * 0.23).clamp(158.0, 230.0);
    let summary = (width - name - status - time).max(170.0);
    [name, summary, status, time]
}

fn home_table_cell(row: egui::Rect, widths: &[f32], index: usize) -> egui::Rect {
    let left = row.left() + widths[..index].iter().sum::<f32>();
    egui::Rect::from_min_size(
        egui::pos2(left, row.top()),
        egui::vec2(widths[index], row.height()),
    )
}

fn paint_home_table_text(
    painter: &egui::Painter,
    cell: egui::Rect,
    text: &str,
    font: egui::FontId,
    color: Color32,
) {
    painter
        .with_clip_rect(cell.shrink2(egui::vec2(7.0, 2.0)))
        .text(
            cell.left_center() + egui::vec2(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            text,
            font,
            color,
        );
}

fn render_home_tool_index(
    ui: &mut egui::Ui,
    categories: &[ToolCategory],
    descriptors: &[ToolDescriptor],
    favorite_tool_ids: &[String],
    actions: &mut Vec<AppAction>,
) {
    ui::card(ui, |ui| {
        ui.columns(categories.len(), |columns| {
            for (index, category) in categories.iter().enumerate() {
                let column = &mut columns[index];
                column.label(
                    RichText::new(category.label())
                        .strong()
                        .size(15.5)
                        .color(ui::palette_for_ui(column).text),
                );
                column.add_space(ui::SPACE_8);
                for descriptor in descriptors
                    .iter()
                    .filter(|descriptor| descriptor.category == *category)
                {
                    let is_favorite = favorite_tool_ids
                        .iter()
                        .any(|tool_id| tool_id == descriptor.id);
                    let (open, toggle_favorite) =
                        home_tool_index_item(column, descriptor, is_favorite);
                    if open {
                        actions.push(AppAction::NavigateTo(descriptor.id.into()));
                    }
                    if toggle_favorite {
                        actions.push(AppAction::ToggleFavorite {
                            tool_id: descriptor.id.into(),
                        });
                    }
                }
            }
        });
    });
}

fn home_tool_index_item(
    ui: &mut egui::Ui,
    descriptor: &ToolDescriptor,
    is_favorite: bool,
) -> (bool, bool) {
    let width = ui.available_width();
    let (response, painter) = ui.allocate_painter(egui::vec2(width, 36.0), egui::Sense::click());
    let palette = ui::palette_for_ui(ui);
    let fill = if response.hovered() {
        palette.hover
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(response.rect, egui::CornerRadius::same(4), fill);
    let icon_rect = egui::Rect::from_min_size(
        response.rect.left_top() + egui::vec2(8.0, 7.0),
        egui::vec2(22.0, 22.0),
    );
    ui::paint_tool_icon(&painter, icon_rect, descriptor.icon, palette.accent);
    painter
        .with_clip_rect(response.rect.shrink2(egui::vec2(38.0, 4.0)))
        .text(
            response.rect.left_center() + egui::vec2(38.0, 0.0),
            egui::Align2::LEFT_CENTER,
            descriptor.name,
            egui::FontId::proportional(13.5),
            palette.text,
        );
    let mut toggle_favorite = false;
    response.context_menu(|ui| {
        if ui
            .button(if is_favorite { "取消收藏" } else { "加入收藏" })
            .clicked()
        {
            toggle_favorite = true;
        }
    });
    (response.clicked(), toggle_favorite)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{actions, navigation, preset_summary, settings_review_presets};
    use crate::{
        settings::SafePresetTool,
        tools::build_registry,
    };

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
    fn settings_review_data_matches_the_reference_presets() {
        let presets = settings_review_presets();

        assert_eq!(presets.len(), 3);
        assert_eq!(presets[0].tool, SafePresetTool::Ping);
        assert_eq!(presets[0].updated_at.as_deref(), Some("2025-05-20 14:32:18"));
        assert_eq!(presets[1].tool, SafePresetTool::TcpProbe);
        assert_eq!(presets[2].tool, SafePresetTool::SerialDebug);
        assert_eq!(
            preset_summary(&presets[2]),
            "波特率=115200, 数据位=8, 校验=无, 停止位=1, 流控=无"
        );
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

    #[test]
    fn port_detail_event_is_tracked_independently_from_port_refresh() {
        let mut latest = HashMap::from([
            ("port-inspector", 1),
            ("port-inspector-process-details", 2),
        ]);

        assert!(actions::accept_task_event(
            &mut latest,
            "port-inspector",
            1,
            true
        ));
        assert_eq!(
            latest.get("port-inspector-process-details"),
            Some(&2)
        );
        assert!(actions::accept_task_event(
            &mut latest,
            "port-inspector-process-details",
            2,
            true
        ));
        assert!(latest.is_empty());
    }
}
