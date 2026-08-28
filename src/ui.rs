//! Windows Toolbox 的共享视觉语言与组件系统。
//! 该模块负责 Windows 中文字体回退、科技感主题令牌、标准化控件（输入框、指标磁贴、各类按钮、徽标、卡片）与矢量图标。

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Frame, Margin, RichText,
    Stroke, TextEdit, Vec2,
};

use crate::tools::ToolIcon;

const FONT_CANDIDATES: [&str; 4] = ["msyh.ttc", "simhei.ttf", "simsun.ttc", "Deng.ttf"];

/// 设计系统间距令牌，单位为 egui point。
pub const SPACE_4: f32 = 4.0;
pub const SPACE_8: f32 = 8.0;
pub const SPACE_12: f32 = 12.0;
pub const SPACE_16: f32 = 16.0;
pub const SPACE_20: f32 = 20.0;
pub const SPACE_24: f32 = 24.0;
pub const SPACE_32: f32 = 32.0;

/// 标准控件高度规范
pub const CONTROL_HEIGHT: f32 = 34.0;
pub const COMPACT_CONTROL_HEIGHT: f32 = 26.0;

/// 工具操作栏在窄窗口中的布局策略，避免输入与按钮互相挤压。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ActionLayout {
    Horizontal,
    Vertical,
}
/// 主题调色板集中管理所有界面的色彩映射，确保对比度与科技感视觉表达。
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct ThemePalette {
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_secondary: Color32,
    pub primary_button: Color32,
    pub primary_button_hover: Color32,
    pub secondary_button_bg: Color32,
    pub secondary_button_hover: Color32,
    pub danger_button: Color32,
    pub danger_button_hover: Color32,
    pub surface: Color32,
    pub surface_elevated: Color32,
    pub panel: Color32,
    pub border: Color32,
    pub border_subtle: Color32,
    pub text: Color32,
    pub weak: Color32,
    pub success_text: Color32,
    pub danger_text: Color32,
    pub warning_text: Color32,
    pub hover: Color32,
}

pub fn theme_palette(theme: egui::Theme) -> ThemePalette {
    let dark = theme == egui::Theme::Dark;
    if dark {
        ThemePalette {
            accent: Color32::from_rgb(0, 225, 255), // 赛博电光青 (Cyber Neon Cyan)
            accent_hover: Color32::from_rgb(56, 235, 255),
            accent_secondary: Color32::from_rgb(129, 140, 248), // 量子紫 (Quantum Violet)
            primary_button: Color32::from_rgb(2, 116, 180),     // 科技深蓝
            primary_button_hover: Color32::from_rgb(3, 136, 209),
            secondary_button_bg: Color32::from_rgb(22, 32, 57),
            secondary_button_hover: Color32::from_rgb(31, 45, 80),
            danger_button: Color32::from_rgb(190, 24, 60), // 激光霓虹红
            danger_button_hover: Color32::from_rgb(225, 29, 72),
            surface: Color32::from_rgb(16, 23, 41), // 黑曜石卡片面
            surface_elevated: Color32::from_rgb(22, 32, 57), // 悬浮科技卡片
            panel: Color32::from_rgb(10, 14, 26),   // 深空主背景/侧边栏
            border: Color32::from_rgb(37, 52, 90),  // 科技边框
            border_subtle: Color32::from_rgb(25, 36, 64),
            text: Color32::from_rgb(241, 245, 249), // 极客钛白
            weak: Color32::from_rgb(150, 168, 196), // 科技银灰
            success_text: Color32::from_rgb(52, 211, 153), // 矩阵翡翠绿
            danger_text: Color32::from_rgb(251, 113, 133), // 警示红
            warning_text: Color32::from_rgb(251, 191, 36), // 琥珀金
            hover: Color32::from_rgb(26, 38, 68),
        }
    } else {
        ThemePalette {
            accent: Color32::from_rgb(2, 116, 180),
            accent_hover: Color32::from_rgb(3, 90, 145),
            accent_secondary: Color32::from_rgb(79, 70, 229),
            primary_button: Color32::from_rgb(2, 116, 180),
            primary_button_hover: Color32::from_rgb(3, 90, 145),
            secondary_button_bg: Color32::from_rgb(241, 245, 249),
            secondary_button_hover: Color32::from_rgb(226, 232, 240),
            danger_button: Color32::from_rgb(190, 24, 60),
            danger_button_hover: Color32::from_rgb(159, 18, 57),
            surface: Color32::WHITE,
            surface_elevated: Color32::from_rgb(248, 250, 252),
            panel: Color32::from_rgb(241, 245, 249),
            border: Color32::from_rgb(203, 213, 225),
            border_subtle: Color32::from_rgb(226, 232, 240),
            text: Color32::from_rgb(15, 23, 42),
            weak: Color32::from_rgb(71, 85, 105),
            success_text: Color32::from_rgb(5, 122, 85),
            danger_text: Color32::from_rgb(190, 24, 60),
            warning_text: Color32::from_rgb(180, 83, 9),
            hover: Color32::from_rgb(224, 242, 254),
        }
    }
}

pub fn action_layout(available_width: f32) -> ActionLayout {
    if available_width >= 680.0 {
        ActionLayout::Horizontal
    } else {
        ActionLayout::Vertical
    }
}

/// 根据 Windows 字体目录构造固定优先级的字体路径。
pub fn font_candidate_paths(font_directory: &Path) -> Vec<PathBuf> {
    FONT_CANDIDATES
        .iter()
        .map(|name| font_directory.join(name))
        .collect()
}

fn first_readable_font(font_directory: &Path) -> Option<(PathBuf, Vec<u8>)> {
    for path in font_candidate_paths(font_directory) {
        match fs::read(&path) {
            Ok(bytes) => return Some((path, bytes)),
            Err(error) if path.exists() => {
                eprintln!("警告：无法读取中文字体 {}：{error}", path.display());
            }
            Err(_) => {}
        }
    }
    None
}

/// 安装 Windows 系统中文字体与符号字体。失败时保留 egui 默认字体，应用仍可正常启动。
pub fn install_windows_fonts(context: &egui::Context) {
    let font_directory = env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts");

    let mut fonts = FontDefinitions::default();

    // 1. 加载 Windows 中文字体 (微软雅黑 / 黑体 / 宋体 / 等线)
    if let Some((path, bytes)) = first_readable_font(&font_directory) {
        let font_name = "windows-chinese-ui".to_owned();
        let mut font_data = FontData::from_owned(bytes);
        font_data.index = 0;
        fonts.font_data.insert(font_name.clone(), font_data.into());
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .insert(0, font_name.clone());
        let monospace = fonts.families.entry(FontFamily::Monospace).or_default();
        let hack_index = monospace
            .iter()
            .position(|name| name == "Hack")
            .map_or(0, |index| index + 1);
        monospace.insert(hack_index, font_name);
        eprintln!("已加载 Windows 中文字体：{}", path.display());
    } else {
        eprintln!("警告：未找到可用 Windows 中文字体，将继续使用 egui 默认字体。");
    }

    // 2. 加载 Windows 原生符号字体 Segoe UI Symbol (包含所有几何图形、星标、箭头与状态字符)
    let symbol_path = font_directory.join("seguisym.ttf");
    if let Ok(bytes) = fs::read(&symbol_path) {
        let sym_name = "windows-symbols".to_owned();
        fonts
            .font_data
            .insert(sym_name.clone(), FontData::from_owned(bytes).into());
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .push(sym_name.clone());
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .push(sym_name);
        eprintln!("已加载 Windows 符号字体：seguisym.ttf");
    }

    // 3. 加载 Windows 原生表情/图标字体 Segoe UI Emoji
    let emoji_path = font_directory.join("seguiemj.ttf");
    if let Ok(bytes) = fs::read(&emoji_path) {
        let emoji_name = "windows-emoji".to_owned();
        fonts
            .font_data
            .insert(emoji_name.clone(), FontData::from_owned(bytes).into());
        fonts
            .families
            .entry(FontFamily::Proportional)
            .or_default()
            .push(emoji_name.clone());
        fonts
            .families
            .entry(FontFamily::Monospace)
            .or_default()
            .push(emoji_name);
        eprintln!("已加载 Windows 表情字体：seguiemj.ttf");
    }

    context.set_fonts(fonts);
}

/// 为浅色与深色主题一次性配置统一的 egui 样式。
pub fn configure_styles(context: &egui::Context) {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        context.style_mut_of(theme, |style| configure_style(style, theme));
    }
}

fn configure_style(style: &mut egui::Style, theme: egui::Theme) {
    let dark = theme == egui::Theme::Dark;
    let palette = theme_palette(theme);

    style.spacing.item_spacing = Vec2::new(SPACE_8, SPACE_8);
    style.spacing.button_padding = Vec2::new(SPACE_12, 7.0);
    style.spacing.window_margin = Margin::same(SPACE_16 as i8);
    style.spacing.menu_margin = Margin::same(SPACE_8 as i8);
    style.spacing.interact_size = Vec2::new(40.0, CONTROL_HEIGHT);

    style
        .text_styles
        .insert(egui::TextStyle::Heading, egui::FontId::proportional(20.0));
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, egui::FontId::monospace(13.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));

    style.visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    style.visuals.panel_fill = palette.panel;
    style.visuals.window_fill = palette.surface;
    style.visuals.extreme_bg_color = palette.surface;
    style.visuals.faint_bg_color = if dark {
        Color32::from_rgb(20, 28, 50)
    } else {
        Color32::from_rgb(246, 249, 253)
    };
    style.visuals.code_bg_color = if dark {
        Color32::from_rgb(22, 32, 57)
    } else {
        Color32::from_rgb(238, 243, 250)
    };
    style.visuals.window_stroke = Stroke::new(1.0_f32, palette.border);
    style.visuals.widgets.noninteractive.bg_fill = palette.surface;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, palette.border);
    style.visuals.widgets.noninteractive.fg_stroke.color = palette.text;
    style.visuals.widgets.noninteractive.corner_radius = CornerRadius::same(6);

    style.visuals.widgets.inactive.bg_fill = palette.surface;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, palette.border);
    style.visuals.widgets.inactive.fg_stroke.color = palette.text;
    style.visuals.widgets.inactive.corner_radius = CornerRadius::same(6);

    style.visuals.widgets.hovered.bg_fill = palette.hover;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.2_f32, palette.accent);
    style.visuals.widgets.hovered.fg_stroke.color = palette.text;
    style.visuals.widgets.hovered.corner_radius = CornerRadius::same(6);

    style.visuals.widgets.active.bg_fill = palette.hover;
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.5_f32, palette.accent);
    style.visuals.widgets.active.fg_stroke.color = palette.text;
    style.visuals.widgets.active.corner_radius = CornerRadius::same(6);

    style.visuals.widgets.open.bg_fill = palette.hover;
    style.visuals.widgets.open.bg_stroke = Stroke::new(1.2_f32, palette.accent);
    style.visuals.widgets.open.corner_radius = CornerRadius::same(6);

    style.visuals.selection.bg_fill = palette
        .accent
        .gamma_multiply(if dark { 0.35 } else { 0.20 });
    style.visuals.selection.stroke = Stroke::new(1.0_f32, palette.accent);
    style.visuals.hyperlink_color = palette.accent;
    style.visuals.weak_text_color = Some(palette.weak);
    style.visuals.window_corner_radius = CornerRadius::same(10);
    style.visuals.menu_corner_radius = CornerRadius::same(8);
}

/// 当前主题的强调色。
pub fn accent(ui: &egui::Ui) -> Color32 {
    ui.visuals().hyperlink_color
}

pub fn palette_for_ui(ui: &egui::Ui) -> ThemePalette {
    theme_palette(if ui.visuals().dark_mode {
        egui::Theme::Dark
    } else {
        egui::Theme::Light
    })
}

/// 返回当前主题中可在页面与卡片背景上阅读的成功状态文字色。
pub fn success_text(ui: &egui::Ui) -> Color32 {
    palette_for_ui(ui).success_text
}

/// 返回当前主题中可在页面与卡片背景上阅读的危险状态文字色。
pub fn danger_text(ui: &egui::Ui) -> Color32 {
    palette_for_ui(ui).danger_text
}

/// 返回当前主题中可在页面与卡片背景上阅读的警告状态文字色。
#[allow(dead_code)]
pub fn warning_text(ui: &egui::Ui) -> Color32 {
    palette_for_ui(ui).warning_text
}

/// 绘制高科技页面标头，包含赛博前缀、标题与描述副标题。
pub fn page_heading(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.horizontal(|ui| {
        let palette = palette_for_ui(ui);
        // 科技感前置指示柱
        let (rect, _) = ui.allocate_exact_size(Vec2::new(4.0, 24.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, CornerRadius::same(2), palette.accent);
        ui.add_space(SPACE_4);
        ui.vertical(|ui| {
            ui.label(RichText::new(title).size(22.0).strong().color(palette.text));
            if !subtitle.is_empty() {
                ui.add_space(1.0);
                ui.label(RichText::new(subtitle).size(13.0).color(palette.weak));
            }
        });
    });
}

/// 绘制标准卡片容器，具有统一的科技圆角、背景与微妙边框。
pub fn card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    let palette = palette_for_ui(ui);
    let frame = Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0_f32, palette.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(SPACE_16 as i8));
    frame.show(ui, add_contents);
}

/// 绘制带有侧边发光高亮条的科技感重点卡片。
pub fn tech_card(
    ui: &mut egui::Ui,
    accent_color: Color32,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    let palette = palette_for_ui(ui);
    let frame = Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0_f32, palette.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(SPACE_16 as i8));
    let response = frame.show(ui, add_contents);
    // 在卡片顶部绘制细长霓虹强调线
    let top_rect = egui::Rect::from_min_size(
        response.response.rect.left_top() + Vec2::new(8.0, 0.0),
        Vec2::new(response.response.rect.width() - 16.0, 2.0),
    );
    ui.painter().rect_filled(
        top_rect,
        CornerRadius::same(1),
        accent_color.gamma_multiply(0.85),
    );
}

/// 绘制高亮/重点卡片，带有微弱的主题背景色。
#[allow(dead_code)]
pub fn elevated_card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    let palette = palette_for_ui(ui);
    let frame = Frame::new()
        .fill(palette.surface_elevated)
        .stroke(Stroke::new(1.0_f32, palette.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(SPACE_16 as i8));
    frame.show(ui, add_contents);
}

/// 绘制按内容收缩的浮层卡片，避免通知提示被扩展到页面全宽。
pub fn compact_card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    let palette = palette_for_ui(ui);
    let frame = Frame::new()
        .fill(palette.surface_elevated)
        .stroke(Stroke::new(1.0_f32, palette.accent.gamma_multiply(0.40)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(SPACE_16 as i8, SPACE_12 as i8));
    frame.show(ui, add_contents);
}

/// 科技数字指标磁贴 (Metric Tile)，用于在控制台顶部展示大字号关键数据。
pub fn metric_tile(
    ui: &mut egui::Ui,
    width: f32,
    label: &str,
    value: &str,
    unit: &str,
    accent_color: Color32,
) {
    let (response, painter) = ui.allocate_painter(Vec2::new(width, 68.0), egui::Sense::hover());
    let palette = palette_for_ui(ui);
    let rect = response.rect;

    // 背景底板与细边框
    let fill_color = if response.hovered() {
        palette.hover
    } else {
        palette.surface
    };
    painter.rect(
        rect,
        CornerRadius::same(6),
        fill_color,
        Stroke::new(1.0_f32, palette.border),
        egui::StrokeKind::Middle,
    );

    // 左侧霓虹发光标记
    let bar_rect = egui::Rect::from_min_size(
        rect.left_top() + Vec2::new(0.0, 8.0),
        Vec2::new(3.0, rect.height() - 16.0),
    );
    painter.rect_filled(bar_rect, CornerRadius::same(1), accent_color);

    // 标签文字
    painter.text(
        rect.left_top() + Vec2::new(12.0, 10.0),
        egui::Align2::LEFT_TOP,
        label,
        egui::FontId::proportional(12.0),
        palette.weak,
    );

    // 主数值
    painter.text(
        rect.left_top() + Vec2::new(12.0, 28.0),
        egui::Align2::LEFT_TOP,
        value,
        egui::FontId::monospace(20.0),
        accent_color,
    );

    // 单位附注
    if !unit.is_empty() {
        let value_width = (value.len() as f32) * 12.0;
        painter.text(
            rect.left_top() + Vec2::new(14.0 + value_width, 36.0),
            egui::Align2::LEFT_TOP,
            unit,
            egui::FontId::proportional(11.0),
            palette.weak,
        );
    }
}

/// 状态指示卡片。
pub fn state_card(ui: &mut egui::Ui, title: &str, detail: &str, color: Color32) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            let (response, painter) = ui.allocate_painter(Vec2::splat(18.0), egui::Sense::hover());
            // 外圈柔光环
            painter.circle_filled(response.rect.center(), 7.0, color.gamma_multiply(0.25));
            // 核心指示灯
            painter.circle_filled(response.rect.center(), 4.0, color);
            ui.add_space(SPACE_4);
            ui.vertical(|ui| {
                ui.label(RichText::new(title).strong().size(14.0));
                if !detail.is_empty() {
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(detail)
                            .size(13.0)
                            .color(ui.visuals().weak_text_color()),
                    );
                }
            });
        });
    });
}

/// 统一创建垂直居中、边距优雅的单行输入框组件。
pub fn text_input<'a>(text: &'a mut String, hint: &'a str) -> TextEdit<'a> {
    TextEdit::singleline(text)
        .hint_text(hint)
        .margin(Margin::symmetric(10, 7))
        .font(egui::TextStyle::Body)
}

/// 标准主按钮（高亮强调色填充，白字，统一科技倒角）。
pub fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let palette = palette_for_ui(ui);
    let button = egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
        .fill(palette.primary_button)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(6))
        .min_size(Vec2::new(0.0, CONTROL_HEIGHT));
    ui.add(button)
}

/// 定制尺寸的标准主按钮。
pub fn primary_button_sized(ui: &mut egui::Ui, text: &str, size: [f32; 2]) -> egui::Response {
    let palette = palette_for_ui(ui);
    let button = egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
        .fill(palette.primary_button)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(6));
    ui.add_sized(size, button)
}

/// 次级按钮（带描边与柔和背景，用于平级操作如“选择文件”、“清除”等）。
pub fn secondary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let palette = palette_for_ui(ui);
    let button = egui::Button::new(RichText::new(text).color(palette.text))
        .fill(palette.secondary_button_bg)
        .stroke(Stroke::new(1.0_f32, palette.border))
        .corner_radius(CornerRadius::same(6))
        .min_size(Vec2::new(0.0, CONTROL_HEIGHT));
    ui.add(button)
}

/// 定制尺寸的次级按钮。
pub fn secondary_button_sized(ui: &mut egui::Ui, text: &str, size: [f32; 2]) -> egui::Response {
    let palette = palette_for_ui(ui);
    let button = egui::Button::new(RichText::new(text).color(palette.text))
        .fill(palette.secondary_button_bg)
        .stroke(Stroke::new(1.0_f32, palette.border))
        .corner_radius(CornerRadius::same(6));
    ui.add_sized(size, button)
}

/// 危险按钮（红色警示，白字，用于“结束进程”等操作）。
pub fn danger_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let palette = palette_for_ui(ui);
    let button = egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
        .fill(palette.danger_button)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(6))
        .min_size(Vec2::new(0.0, CONTROL_HEIGHT));
    ui.add(button)
}

/// 定制尺寸的危险按钮。
#[allow(dead_code)]
pub fn danger_button_sized(ui: &mut egui::Ui, text: &str, size: [f32; 2]) -> egui::Response {
    let palette = palette_for_ui(ui);
    let button = egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
        .fill(palette.danger_button)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(6));
    ui.add_sized(size, button)
}

/// 紧凑操作小按钮（用于表格行内、卡片内“复制”、“打开所在位置”等操作）。
pub fn small_action_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let palette = palette_for_ui(ui);
    let button = egui::Button::new(RichText::new(text).size(12.0).color(palette.text))
        .fill(palette.secondary_button_bg)
        .stroke(Stroke::new(1.0_f32, palette.border_subtle))
        .corner_radius(CornerRadius::same(4))
        .min_size(Vec2::new(0.0, COMPACT_CONTROL_HEIGHT));
    ui.add(button)
}

/// 状态徽标 / 胶囊标签（用于协议 TCP/UDP、状态 LISTENING/ESTABLISHED、PID 标签等）。
pub fn badge(ui: &mut egui::Ui, text: &str, fg: Color32, bg: Color32) -> egui::Response {
    let font_id = egui::FontId::proportional(11.5);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font_id, fg);
    let size = Vec2::new(galley.size().x + 14.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    painter.rect(
        rect,
        CornerRadius::same(11),
        bg,
        Stroke::new(1.0_f32, fg.gamma_multiply(0.35)),
        egui::StrokeKind::Middle,
    );
    painter.galley(
        rect.left_center() + Vec2::new(7.0, -galley.size().y * 0.5),
        galley,
        fg,
    );
    response
}

/// 带有微型状态脉冲小圆点的科技徽章。
pub fn status_pill(ui: &mut egui::Ui, text: &str, color: Color32) -> egui::Response {
    let font_id = egui::FontId::proportional(12.0);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font_id, color);
    let size = Vec2::new(galley.size().x + 24.0, 22.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter();
    let bg = color.gamma_multiply(if ui.visuals().dark_mode { 0.16 } else { 0.10 });
    painter.rect(
        rect,
        CornerRadius::same(11),
        bg,
        Stroke::new(1.0_f32, color.gamma_multiply(0.40)),
        egui::StrokeKind::Middle,
    );
    // 脉冲小圆点
    let dot_center = rect.left_center() + Vec2::new(9.0, 0.0);
    painter.circle_filled(dot_center, 4.5, color.gamma_multiply(0.30));
    painter.circle_filled(dot_center, 2.5, color);

    painter.galley(
        rect.left_center() + Vec2::new(17.0, -galley.size().y * 0.5),
        galley,
        color,
    );
    response
}

/// 现代图标徽章容器，绘制带彩色底色的图标。
pub fn icon_badge(
    ui: &mut egui::Ui,
    icon: ToolIcon,
    size: f32,
    icon_color: Color32,
    bg_color: Color32,
) {
    let (response, painter) = ui.allocate_painter(Vec2::splat(size), egui::Sense::hover());
    painter.rect(
        response.rect,
        CornerRadius::same(8),
        bg_color,
        Stroke::new(1.0_f32, icon_color.gamma_multiply(0.30)),
        egui::StrokeKind::Middle,
    );
    paint_tool_icon(&painter, response.rect, icon, icon_color);
}

/// 在固定方形区域内绘制一致的线性工具图标。
#[allow(dead_code)]
pub fn tool_icon(ui: &mut egui::Ui, icon: ToolIcon, size: f32, color: Color32) -> egui::Response {
    let (response, painter) = ui.allocate_painter(Vec2::splat(size), egui::Sense::hover());
    paint_tool_icon(&painter, response.rect, icon, color);
    response
}

/// 在指定矩形内绘制图标，不参与布局，供卡片与导航的自定义绘制复用。
pub fn paint_tool_icon(
    painter: &egui::Painter,
    bounds: egui::Rect,
    icon: ToolIcon,
    color: Color32,
) {
    let size = bounds.width().min(bounds.height());
    let rect = egui::Rect::from_center_size(bounds.center(), Vec2::splat(size * 0.60));
    let stroke = Stroke::new((size * 0.075).max(1.3), color);
    match icon {
        ToolIcon::FileLock => {
            painter.rect_stroke(
                rect,
                CornerRadius::same(3),
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.line_segment(
                [
                    rect.left_top() + Vec2::new(size * 0.42, 0.0),
                    rect.right_top(),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    rect.right_top(),
                    rect.right_top() + Vec2::new(0.0, size * 0.28),
                ],
                stroke,
            );
            let center = rect.center() + Vec2::new(0.0, size * 0.09);
            painter.circle_stroke(center, size * 0.13, stroke);
            painter.line_segment(
                [
                    center + Vec2::new(size * 0.09, size * 0.09),
                    center + Vec2::new(size * 0.20, size * 0.20),
                ],
                stroke,
            );
        }
        ToolIcon::Network => {
            let center = rect.center();
            let points = [
                rect.left_center(),
                rect.right_center(),
                rect.center_top(),
                rect.center_bottom(),
            ];
            for point in points {
                painter.line_segment([center, point], stroke);
                painter.circle_filled(point, size * 0.075, color);
            }
            painter.circle_filled(center, size * 0.095, color);
        }
        ToolIcon::Process => {
            let top = rect.center_top() + Vec2::new(0.0, size * 0.08);
            let left = rect.left_bottom() + Vec2::new(size * 0.10, -size * 0.08);
            let right = rect.right_bottom() + Vec2::new(-size * 0.10, -size * 0.08);
            painter.line_segment([top, left], stroke);
            painter.line_segment([top, right], stroke);
            painter.circle_filled(top, size * 0.09, color);
            painter.circle_filled(left, size * 0.09, color);
            painter.circle_filled(right, size * 0.09, color);
        }
        ToolIcon::Dns => {
            painter.circle_stroke(rect.center(), size * 0.28, stroke);
            painter.line_segment([rect.left_center(), rect.right_center()], stroke);
            painter.line_segment([rect.center_top(), rect.center_bottom()], stroke);
            // 绘制小型雷达脉冲点
            painter.circle_filled(
                rect.center() + Vec2::new(size * 0.10, -size * 0.10),
                size * 0.06,
                color,
            );
        }
        ToolIcon::Ping => {
            // 脉冲波形图
            let center = rect.center();
            painter.circle_stroke(center, size * 0.12, stroke);
            painter.circle_stroke(
                center,
                size * 0.24,
                Stroke::new((size * 0.06).max(1.0), color.gamma_multiply(0.60)),
            );
            painter.circle_stroke(
                center,
                size * 0.32,
                Stroke::new((size * 0.05).max(0.8), color.gamma_multiply(0.30)),
            );
            painter.circle_filled(center, size * 0.06, color);
        }
        ToolIcon::TcpProbe => {
            // 握手双向连接图
            let left = rect.left_center() + Vec2::new(size * 0.05, 0.0);
            let right = rect.right_center() - Vec2::new(size * 0.05, 0.0);
            painter.line_segment([left, left + Vec2::new(size * 0.18, 0.0)], stroke);
            painter.line_segment([right, right - Vec2::new(size * 0.18, 0.0)], stroke);
            painter.circle_filled(left, size * 0.08, color);
            painter.circle_filled(right, size * 0.08, color);
            let center_rect = egui::Rect::from_center_size(rect.center(), Vec2::splat(size * 0.22));
            painter.rect_stroke(
                center_rect,
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        ToolIcon::Mtr => {
            // 逐跳路由拓扑图
            let p1 = rect.left_bottom() + Vec2::new(size * 0.06, -size * 0.06);
            let p2 = rect.center() + Vec2::new(-size * 0.04, -size * 0.12);
            let p3 = rect.right_bottom() + Vec2::new(-size * 0.06, -size * 0.06);
            painter.line_segment([p1, p2], stroke);
            painter.line_segment([p2, p3], stroke);
            painter.circle_filled(p1, size * 0.08, color);
            painter.circle_filled(p2, size * 0.09, color);
            painter.circle_filled(p3, size * 0.08, color);
        }
        ToolIcon::TcpDebug => {
            // 两个端点与双向箭头表示可持续双向 TCP 字节流。
            let left = rect.left_center() + Vec2::new(size * 0.04, 0.0);
            let right = rect.right_center() - Vec2::new(size * 0.04, 0.0);
            painter.rect_stroke(
                egui::Rect::from_center_size(left, Vec2::splat(size * 0.18)),
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.rect_stroke(
                egui::Rect::from_center_size(right, Vec2::splat(size * 0.18)),
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.arrow(
                left + Vec2::new(size * 0.12, -size * 0.08),
                Vec2::new(size * 0.25, 0.0),
                stroke,
            );
            painter.arrow(
                right - Vec2::new(size * 0.12, -size * 0.08),
                Vec2::new(-size * 0.25, 0.0),
                stroke,
            );
        }
        ToolIcon::UdpDebug => {
            // 中心数据报向多个端点发散，区别于 TCP 的持续连接。
            let center = rect.center();
            painter.circle_stroke(center, size * 0.10, stroke);
            for offset in [
                Vec2::new(-size * 0.24, -size * 0.20),
                Vec2::new(size * 0.24, -size * 0.20),
                Vec2::new(0.0, size * 0.28),
            ] {
                let endpoint = center + offset;
                painter.arrow(center + offset * 0.35, offset * 0.50, stroke);
                painter.circle_filled(endpoint, size * 0.065, color);
            }
        }
        ToolIcon::SerialDebug => {
            // DB9 风格轮廓与引脚阵列表示串口设备。
            let connector =
                egui::Rect::from_center_size(rect.center(), Vec2::new(size * 0.48, size * 0.34));
            painter.rect_stroke(
                connector,
                CornerRadius::same(3),
                stroke,
                egui::StrokeKind::Middle,
            );
            for row in 0..2 {
                for column in 0..3 {
                    let x = connector.left() + size * (0.12 + column as f32 * 0.12);
                    let y = connector.top() + size * (0.10 + row as f32 * 0.14);
                    painter.circle_filled(egui::pos2(x, y), size * 0.035, color);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ActionLayout, action_layout, first_readable_font, font_candidate_paths, theme_palette,
    };
    use eframe::egui::{Color32, Theme};
    use std::path::Path;

    fn relative_luminance(color: Color32) -> f32 {
        fn channel(value: u8) -> f32 {
            let value = f32::from(value) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }

    fn contrast_ratio(first: Color32, second: Color32) -> f32 {
        let (lighter, darker) = (relative_luminance(first), relative_luminance(second));
        let (lighter, darker) = if lighter >= darker {
            (lighter, darker)
        } else {
            (darker, lighter)
        };
        (lighter + 0.05) / (darker + 0.05)
    }

    #[test]
    fn font_candidates_follow_windows_ui_priority() {
        let paths = font_candidate_paths(Path::new("C:/Windows/Fonts"));
        let names: Vec<_> = paths
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert_eq!(names, ["msyh.ttc", "simhei.ttf", "simsun.ttc", "Deng.ttf"]);
    }

    #[test]
    fn missing_font_candidates_have_no_fallback_path() {
        assert!(first_readable_font(Path::new("Z:/missing-fonts")).is_none());
    }

    #[test]
    fn action_layout_switches_at_compact_breakpoint() {
        assert_eq!(action_layout(680.0), ActionLayout::Horizontal);
        assert_eq!(action_layout(679.0), ActionLayout::Vertical);
    }

    #[test]
    fn theme_text_buttons_and_focus_meet_contrast_targets() {
        for theme in [Theme::Light, Theme::Dark] {
            let palette = theme_palette(theme);
            for background in [palette.panel, palette.surface] {
                assert!(contrast_ratio(palette.text, background) >= 4.5);
                assert!(contrast_ratio(palette.weak, background) >= 4.5);
                assert!(contrast_ratio(palette.success_text, background) >= 4.5);
                assert!(contrast_ratio(palette.danger_text, background) >= 4.5);
                assert!(contrast_ratio(palette.accent, background) >= 3.0);
            }
            assert!(contrast_ratio(Color32::WHITE, palette.primary_button) >= 4.5);
            assert!(contrast_ratio(Color32::WHITE, palette.danger_button) >= 4.5);
        }
    }
}
