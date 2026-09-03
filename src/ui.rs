//! ToolDeck 的共享视觉语言与组件系统。
//! 该模块负责 Windows 中文字体、工业工作台主题令牌、标准控件和无文字矢量图标。

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
const MONO_FONT_CANDIDATES: [&str; 3] = ["CascadiaMono.ttf", "CascadiaCode.ttf", "consola.ttf"];

/// 设计系统间距令牌，单位为 egui point。
pub const SPACE_4: f32 = 4.0;
pub const SPACE_8: f32 = 8.0;
pub const SPACE_12: f32 = 12.0;
pub const SPACE_16: f32 = 16.0;

/// 页面内容的标准横向留白，单位为 egui point。
pub const PAGE_PADDING: f32 = 16.0;

/// 标准控件高度规范
pub const CONTROL_HEIGHT: f32 = 34.0;
pub const COMPACT_CONTROL_HEIGHT: f32 = 26.0;

/// 应用外壳与通用操作使用的无文字矢量图标。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AppIcon {
    Home,
    Network,
    System,
    Developer,
    Settings,
    About,
    Back,
    Copy,
    Export,
    Clear,
    Pause,
}

/// 主题调色板集中管理所有界面的色彩映射，确保对比度与科技感视觉表达。
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub struct ThemePalette {
    pub accent: Color32,
    pub receive_text: Color32,
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
            accent: Color32::from_rgb(38, 183, 232),
            receive_text: Color32::from_rgb(59, 167, 255),
            accent_hover: Color32::from_rgb(92, 205, 243),
            accent_secondary: Color32::from_rgb(108, 167, 194),
            primary_button: Color32::from_rgb(0, 125, 158),
            primary_button_hover: Color32::from_rgb(0, 111, 142),
            secondary_button_bg: Color32::from_rgb(18, 30, 39),
            secondary_button_hover: Color32::from_rgb(27, 43, 54),
            danger_button: Color32::from_rgb(181, 51, 59),
            danger_button_hover: Color32::from_rgb(151, 40, 48),
            surface: Color32::from_rgb(13, 23, 30),
            surface_elevated: Color32::from_rgb(18, 30, 39),
            panel: Color32::from_rgb(9, 16, 22),
            border: Color32::from_rgb(38, 49, 58),
            border_subtle: Color32::from_rgb(38, 49, 58),
            text: Color32::from_rgb(230, 237, 243),
            weak: Color32::from_rgb(147, 162, 175),
            success_text: Color32::from_rgb(67, 201, 107),
            danger_text: Color32::from_rgb(240, 91, 97),
            warning_text: Color32::from_rgb(239, 182, 69),
            hover: Color32::from_rgb(20, 43, 56),
        }
    } else {
        ThemePalette {
            accent: Color32::from_rgb(0, 125, 158),
            receive_text: Color32::from_rgb(0, 111, 196),
            accent_hover: Color32::from_rgb(0, 101, 132),
            accent_secondary: Color32::from_rgb(51, 112, 132),
            primary_button: Color32::from_rgb(0, 105, 137),
            primary_button_hover: Color32::from_rgb(0, 91, 120),
            secondary_button_bg: Color32::from_rgb(245, 247, 248),
            secondary_button_hover: Color32::from_rgb(234, 239, 241),
            danger_button: Color32::from_rgb(181, 51, 59),
            danger_button_hover: Color32::from_rgb(151, 40, 48),
            surface: Color32::WHITE,
            surface_elevated: Color32::from_rgb(248, 250, 250),
            panel: Color32::from_rgb(239, 243, 244),
            border: Color32::from_rgb(190, 201, 205),
            border_subtle: Color32::from_rgb(220, 227, 229),
            text: Color32::from_rgb(25, 35, 40),
            weak: Color32::from_rgb(76, 91, 98),
            success_text: Color32::from_rgb(32, 126, 61),
            danger_text: Color32::from_rgb(174, 47, 54),
            warning_text: Color32::from_rgb(151, 101, 8),
            hover: Color32::from_rgb(224, 239, 242),
        }
    }
}

/// 根据 Windows 字体目录构造固定优先级的字体路径。
pub fn font_candidate_paths(font_directory: &Path) -> Vec<PathBuf> {
    FONT_CANDIDATES
        .iter()
        .map(|name| font_directory.join(name))
        .collect()
}

pub fn monospace_font_candidate_paths(font_directory: &Path) -> Vec<PathBuf> {
    MONO_FONT_CANDIDATES
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

    for path in monospace_font_candidate_paths(&font_directory) {
        if let Ok(bytes) = fs::read(&path) {
            let font_name = "windows-data-mono".to_owned();
            fonts
                .font_data
                .insert(font_name.clone(), FontData::from_owned(bytes).into());
            fonts
                .families
                .entry(FontFamily::Monospace)
                .or_default()
                .insert(0, font_name);
            break;
        }
    }

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
        .insert(egui::TextStyle::Body, egui::FontId::proportional(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, egui::FontId::monospace(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(13.0));

    style.visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    style.visuals.panel_fill = palette.panel;
    style.visuals.window_fill = palette.surface;
    style.visuals.extreme_bg_color = palette.surface;
    style.visuals.faint_bg_color = if dark {
        Color32::from_rgb(16, 28, 36)
    } else {
        Color32::from_rgb(246, 248, 249)
    };
    style.visuals.code_bg_color = if dark {
        Color32::from_rgb(15, 27, 35)
    } else {
        Color32::from_rgb(239, 243, 244)
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
    style.visuals.window_corner_radius = CornerRadius::same(6);
    style.visuals.menu_corner_radius = CornerRadius::same(6);
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

/// 绘制工作台页面标头，标题与说明使用独立基线。
pub fn page_heading(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    let palette = palette_for_ui(ui);
    ui.vertical(|ui| {
        ui.label(RichText::new(title).size(26.0).strong().color(palette.text));
        if !subtitle.is_empty() {
            ui.add_space(2.0);
            ui.label(RichText::new(subtitle).size(15.0).color(palette.weak));
        }
    });
}

/// 绘制标准工具容器；仅用于确实需要边界的重复项目或工具区。
pub fn card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    let palette = palette_for_ui(ui);
    let frame = Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0_f32, palette.border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(SPACE_12 as i8));
    frame.show(ui, add_contents);
}

/// 绘制表格工作区容器；表头和数据行从边框内侧开始，保持设计要求的高信息密度。
pub fn table_card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    let palette = palette_for_ui(ui);
    let frame = Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0_f32, palette.border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::ZERO);
    frame.show(ui, add_contents);
}

/// 绘制平面页面分区，通过底部分隔线建立层级，避免页面套卡片。
pub fn section(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    Frame::new()
        .fill(Color32::TRANSPARENT)
        .inner_margin(Margin::symmetric(0, SPACE_12 as i8))
        .show(ui, add_contents);
    ui.separator();
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

/// 绘制带文字标签的二元开关，尺寸与通信页面设计图中的控制开关保持一致。
pub fn toggle_switch(ui: &mut egui::Ui, value: &mut bool, label: &str) -> egui::Response {
    let palette = palette_for_ui(ui);
    let font_id = egui::FontId::proportional(14.0);
    let label_galley =
        ui.fonts(|fonts| fonts.layout_no_wrap(label.to_owned(), font_id.clone(), palette.text));
    let switch_size = Vec2::new(38.0, 20.0);
    let label_spacing = if label.is_empty() { 0.0 } else { SPACE_8 };
    let desired_size = Vec2::new(
        label_galley.size().x + label_spacing + switch_size.x,
        switch_size.y.max(label_galley.size().y),
    );
    let (rect, mut response) = ui.allocate_exact_size(desired_size, egui::Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }

    let enabled = ui.is_enabled();
    let track_rect = egui::Rect::from_min_size(
        egui::pos2(
            rect.left() + label_galley.size().x + label_spacing,
            rect.center().y - switch_size.y * 0.5,
        ),
        switch_size,
    );
    let track_color = if *value {
        palette.accent
    } else {
        palette.border
    }
    .gamma_multiply(if enabled { 1.0 } else { 0.45 });
    let knob_x = if *value {
        track_rect.right() - 10.0
    } else {
        track_rect.left() + 10.0
    };
    ui.painter()
        .rect_filled(track_rect, CornerRadius::same(10), track_color);
    ui.painter().circle_filled(
        egui::pos2(knob_x, track_rect.center().y),
        7.0,
        Color32::WHITE.gamma_multiply(if enabled { 1.0 } else { 0.55 }),
    );
    if !label.is_empty() {
        let label_pos = egui::pos2(rect.left(), rect.center().y - label_galley.size().y * 0.5);
        ui.painter().galley(
            label_pos,
            label_galley,
            palette
                .text
                .gamma_multiply(if enabled { 1.0 } else { 0.45 }),
        );
    }
    response
}

/// 绘制进程树等层级控件使用的无字符展开按钮。
pub fn disclosure_button(ui: &mut egui::Ui, expanded: bool) -> egui::Response {
    let palette = palette_for_ui(ui);
    let (rect, response) = ui.allocate_exact_size(Vec2::new(20.0, 24.0), egui::Sense::click());
    let color = if response.hovered() || response.has_focus() {
        palette.accent
    } else {
        palette.weak
    };
    let center = rect.center();
    let stroke = Stroke::new(1.6_f32, color);
    if expanded {
        ui.painter().line_segment(
            [center + Vec2::new(-4.0, -2.0), center + Vec2::new(0.0, 2.0)],
            stroke,
        );
        ui.painter().line_segment(
            [center + Vec2::new(0.0, 2.0), center + Vec2::new(4.0, -2.0)],
            stroke,
        );
    } else {
        ui.painter().line_segment(
            [center + Vec2::new(-2.0, -4.0), center + Vec2::new(2.0, 0.0)],
            stroke,
        );
        ui.painter().line_segment(
            [center + Vec2::new(2.0, 0.0), center + Vec2::new(-2.0, 4.0)],
            stroke,
        );
    }
    response.on_hover_text(if expanded { "收起" } else { "展开" })
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

/// 发送类主要操作使用绿色，和接收状态的青蓝色形成稳定区分。
pub fn success_button_sized(ui: &mut egui::Ui, text: &str, size: [f32; 2]) -> egui::Response {
    let palette = palette_for_ui(ui);
    let button = egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
        .fill(palette.success_text)
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

/// 绘制固定尺寸的无文字图标按钮，tooltip 由调用方提供可读名称。
pub fn icon_button(
    ui: &mut egui::Ui,
    icon: AppIcon,
    tooltip: &str,
    selected: bool,
) -> egui::Response {
    icon_button_sized(ui, icon, tooltip, selected, 36.0)
}

/// 绘制可指定尺寸的无文字图标按钮。
pub fn icon_button_sized(
    ui: &mut egui::Ui,
    icon: AppIcon,
    tooltip: &str,
    selected: bool,
    size: f32,
) -> egui::Response {
    let palette = palette_for_ui(ui);
    let (rect, mut response) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::click());
    let fill = if selected {
        palette
            .accent
            .gamma_multiply(if ui.visuals().dark_mode { 0.18 } else { 0.11 })
    } else if response.hovered() {
        palette.hover
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
    if selected || response.has_focus() {
        ui.painter().rect_stroke(
            rect.shrink(0.5),
            CornerRadius::same(5),
            Stroke::new(1.0_f32, palette.accent),
            egui::StrokeKind::Middle,
        );
    }
    paint_app_icon(
        ui.painter(),
        rect.shrink(size * 0.23),
        icon,
        if selected {
            palette.accent
        } else {
            palette.weak
        },
    );
    if response.clicked() {
        response.request_focus();
    }
    response = response.on_hover_text(tooltip);
    response
}

/// 在指定矩形内绘制应用级线性图标。
pub fn paint_app_icon(painter: &egui::Painter, bounds: egui::Rect, icon: AppIcon, color: Color32) {
    let size = bounds.width().min(bounds.height());
    let rect = egui::Rect::from_center_size(bounds.center(), Vec2::splat(size));
    let stroke = Stroke::new((size * 0.09).max(1.4), color);
    let center = rect.center();
    match icon {
        AppIcon::Home => {
            let roof_left = rect.left_center() + Vec2::new(0.0, -size * 0.08);
            let roof_top = rect.center_top();
            let roof_right = rect.right_center() + Vec2::new(0.0, -size * 0.08);
            painter.line_segment([roof_left, roof_top], stroke);
            painter.line_segment([roof_top, roof_right], stroke);
            painter.line_segment([roof_left, rect.left_bottom()], stroke);
            painter.line_segment([roof_right, rect.right_bottom()], stroke);
            painter.line_segment([rect.left_bottom(), rect.right_bottom()], stroke);
        }
        AppIcon::Network => {
            let top = rect.center_top() + Vec2::new(0.0, size * 0.08);
            let left = rect.left_bottom() + Vec2::new(size * 0.08, -size * 0.08);
            let right = rect.right_bottom() + Vec2::new(-size * 0.08, -size * 0.08);
            painter.line_segment([top, left], stroke);
            painter.line_segment([top, right], stroke);
            painter.line_segment([left, right], stroke);
            for point in [top, left, right] {
                painter.circle_filled(point, size * 0.10, color);
            }
        }
        AppIcon::System => {
            painter.rect_stroke(
                rect.shrink(size * 0.08),
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x, rect.bottom() - size * 0.08),
                    egui::pos2(center.x, rect.bottom() + size * 0.10),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    egui::pos2(center.x - size * 0.24, rect.bottom() + size * 0.10),
                    egui::pos2(center.x + size * 0.24, rect.bottom() + size * 0.10),
                ],
                stroke,
            );
        }
        AppIcon::Developer => {
            painter.line_segment(
                [
                    center + Vec2::new(-size * 0.10, -size * 0.36),
                    center + Vec2::new(-size * 0.36, 0.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + Vec2::new(-size * 0.36, 0.0),
                    center + Vec2::new(-size * 0.10, size * 0.36),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + Vec2::new(size * 0.10, -size * 0.36),
                    center + Vec2::new(size * 0.36, 0.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + Vec2::new(size * 0.36, 0.0),
                    center + Vec2::new(size * 0.10, size * 0.36),
                ],
                stroke,
            );
        }
        AppIcon::Settings => {
            painter.circle_stroke(center, size * 0.22, stroke);
            painter.circle_stroke(center, size * 0.07, stroke);
            for direction in [
                Vec2::new(1.0, 0.0),
                Vec2::new(-1.0, 0.0),
                Vec2::new(0.0, 1.0),
                Vec2::new(0.0, -1.0),
            ] {
                painter.line_segment(
                    [
                        center + direction * size * 0.26,
                        center + direction * size * 0.42,
                    ],
                    stroke,
                );
            }
        }
        AppIcon::About => {
            painter.circle_stroke(center, size * 0.42, stroke);
            painter.circle_filled(center + Vec2::new(0.0, -size * 0.20), size * 0.06, color);
            painter.line_segment(
                [
                    center + Vec2::new(0.0, -size * 0.03),
                    center + Vec2::new(0.0, size * 0.24),
                ],
                stroke,
            );
        }
        AppIcon::Back => {
            painter.line_segment(
                [
                    center + Vec2::new(-size * 0.34, 0.0),
                    center + Vec2::new(size * 0.34, 0.0),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + Vec2::new(-size * 0.34, 0.0),
                    center + Vec2::new(-size * 0.08, -size * 0.26),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + Vec2::new(-size * 0.34, 0.0),
                    center + Vec2::new(-size * 0.08, size * 0.26),
                ],
                stroke,
            );
        }
        AppIcon::Copy => {
            let first = rect
                .shrink(size * 0.14)
                .translate(Vec2::new(-size * 0.08, -size * 0.08));
            let second = rect
                .shrink(size * 0.14)
                .translate(Vec2::new(size * 0.08, size * 0.08));
            painter.rect_stroke(
                first,
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.rect_stroke(
                second,
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Middle,
            );
        }
        AppIcon::Export => {
            painter.line_segment(
                [
                    center + Vec2::new(0.0, size * 0.30),
                    center + Vec2::new(0.0, -size * 0.34),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + Vec2::new(0.0, -size * 0.34),
                    center + Vec2::new(-size * 0.20, -size * 0.14),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + Vec2::new(0.0, -size * 0.34),
                    center + Vec2::new(size * 0.20, -size * 0.14),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + Vec2::new(-size * 0.36, size * 0.30),
                    center + Vec2::new(size * 0.36, size * 0.30),
                ],
                stroke,
            );
        }
        AppIcon::Clear => {
            let body = egui::Rect::from_center_size(
                center + Vec2::new(0.0, size * 0.08),
                Vec2::new(size * 0.52, size * 0.58),
            );
            painter.rect_stroke(
                body,
                CornerRadius::same(2),
                stroke,
                egui::StrokeKind::Middle,
            );
            painter.line_segment(
                [
                    center + Vec2::new(-size * 0.34, -size * 0.28),
                    center + Vec2::new(size * 0.34, -size * 0.28),
                ],
                stroke,
            );
        }
        AppIcon::Pause => {
            for offset in [-size * 0.16, size * 0.16] {
                painter.line_segment(
                    [
                        center + Vec2::new(offset, -size * 0.30),
                        center + Vec2::new(offset, size * 0.30),
                    ],
                    stroke,
                );
            }
        }
    }
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
        first_readable_font, font_candidate_paths, monospace_font_candidate_paths, theme_palette,
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
    fn data_font_candidates_prioritize_cascadia_mono_then_consolas() {
        let paths = monospace_font_candidate_paths(Path::new("C:/Windows/Fonts"));
        let names = paths
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            ["CascadiaMono.ttf", "CascadiaCode.ttf", "consola.ttf"]
        );
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
