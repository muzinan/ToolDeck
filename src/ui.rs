//! Windows Toolbox 的共享视觉语言。
//! 该模块负责 Windows 中文字体回退、浅深色主题令牌、通用状态组件和矢量图标；不参与工具业务逻辑。

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, Frame, Margin, RichText,
    Stroke, Vec2,
};

use crate::tools::ToolIcon;

const FONT_CANDIDATES: [&str; 4] = ["msyh.ttc", "simhei.ttf", "simsun.ttc", "Deng.ttf"];

/// 设计系统间距令牌，单位为 egui point。
pub const SPACE_4: f32 = 4.0;
pub const SPACE_8: f32 = 8.0;
pub const SPACE_12: f32 = 12.0;
pub const SPACE_16: f32 = 16.0;
pub const SPACE_24: f32 = 24.0;

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

/// 安装 Windows 系统中文字体。失败时保留 egui 默认字体，应用仍可正常启动。
pub fn install_windows_fonts(context: &egui::Context) {
    let font_directory = env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts");
    let Some((path, bytes)) = first_readable_font(&font_directory) else {
        eprintln!("警告：未找到可用 Windows 中文字体，将继续使用 egui 默认字体。");
        return;
    };

    let mut fonts = FontDefinitions::default();
    let font_name = "windows-chinese-ui".to_owned();
    let mut font_data = FontData::from_owned(bytes);
    // TTC 字体集合选择第一个字体面；单字体文件的索引同样为 0。
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
    context.set_fonts(fonts);
    eprintln!("已加载 Windows 中文字体：{}", path.display());
}

/// 为浅色与深色主题一次性配置统一的 egui 样式。
pub fn configure_styles(context: &egui::Context) {
    for theme in [egui::Theme::Light, egui::Theme::Dark] {
        context.style_mut_of(theme, |style| configure_style(style, theme));
    }
}

fn configure_style(style: &mut egui::Style, theme: egui::Theme) {
    let dark = theme == egui::Theme::Dark;
    let accent = if dark {
        Color32::from_rgb(91, 140, 255)
    } else {
        Color32::from_rgb(37, 99, 235)
    };
    let surface = if dark {
        Color32::from_rgb(21, 29, 43)
    } else {
        Color32::WHITE
    };
    let panel = if dark {
        Color32::from_rgb(14, 20, 31)
    } else {
        Color32::from_rgb(242, 246, 252)
    };
    let border = if dark {
        Color32::from_rgb(50, 64, 85)
    } else {
        Color32::from_rgb(214, 224, 238)
    };
    let text = if dark {
        Color32::from_rgb(232, 238, 249)
    } else {
        Color32::from_rgb(25, 35, 52)
    };
    let weak = if dark {
        Color32::from_rgb(164, 178, 199)
    } else {
        Color32::from_rgb(90, 107, 132)
    };
    let hover = if dark {
        Color32::from_rgb(36, 52, 78)
    } else {
        Color32::from_rgb(232, 241, 255)
    };

    style.spacing.item_spacing = Vec2::new(SPACE_8, SPACE_8);
    style.spacing.button_padding = Vec2::new(SPACE_12, SPACE_8);
    style.spacing.window_margin = Margin::same(SPACE_16 as i8);
    style.spacing.menu_margin = Margin::same(SPACE_8 as i8);
    style.spacing.interact_size = Vec2::new(40.0, 34.0);
    style
        .text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
    style.visuals = if dark {
        egui::Visuals::dark()
    } else {
        egui::Visuals::light()
    };
    style.visuals.panel_fill = panel;
    style.visuals.window_fill = surface;
    style.visuals.extreme_bg_color = surface;
    style.visuals.faint_bg_color = if dark {
        Color32::from_rgb(26, 37, 54)
    } else {
        Color32::from_rgb(247, 249, 253)
    };
    style.visuals.code_bg_color = if dark {
        Color32::from_rgb(25, 35, 52)
    } else {
        Color32::from_rgb(238, 243, 250)
    };
    style.visuals.window_stroke = Stroke::new(1.0_f32, border);
    style.visuals.widgets.noninteractive.bg_fill = surface;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, border);
    style.visuals.widgets.noninteractive.fg_stroke.color = text;
    style.visuals.widgets.inactive.bg_fill = surface;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, border);
    style.visuals.widgets.inactive.fg_stroke.color = text;
    style.visuals.widgets.hovered.bg_fill = hover;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.5_f32, accent);
    style.visuals.widgets.hovered.fg_stroke.color = text;
    style.visuals.widgets.active.bg_fill = accent;
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, accent);
    style.visuals.widgets.active.fg_stroke.color = Color32::WHITE;
    style.visuals.widgets.open.bg_fill = hover;
    style.visuals.widgets.open.bg_stroke = Stroke::new(1.0_f32, accent);
    style.visuals.selection.bg_fill = accent.gamma_multiply(if dark { 0.45 } else { 0.22 });
    style.visuals.selection.stroke = Stroke::new(1.0_f32, accent);
    style.visuals.hyperlink_color = accent;
    style.visuals.weak_text_color = Some(weak);
    style.visuals.window_corner_radius = CornerRadius::same(12);
    style.visuals.menu_corner_radius = CornerRadius::same(8);
    style.visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
    style.visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
    style.visuals.widgets.active.corner_radius = CornerRadius::same(6);
    style.visuals.widgets.open.corner_radius = CornerRadius::same(6);
}

/// 当前主题的强调色。
pub fn accent(ui: &egui::Ui) -> Color32 {
    ui.visuals().hyperlink_color
}

pub fn success() -> Color32 {
    Color32::from_rgb(22, 138, 91)
}
pub fn danger() -> Color32 {
    Color32::from_rgb(205, 67, 67)
}

/// 页面标题与说明，统一所有工具页面的信息层级。
pub fn page_heading(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).size(26.0).strong());
    ui.add_space(SPACE_4);
    ui.label(RichText::new(subtitle).color(ui.visuals().weak_text_color()));
}

/// 绘制填满当前内容宽度的页面级信息卡片。
pub fn card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    let content_width = (ui.available_width() - SPACE_16 * 2.0).max(0.0);
    let frame = Frame::new()
        .fill(ui.visuals().window_fill)
        .stroke(Stroke::new(
            1.0_f32,
            ui.visuals().widgets.inactive.bg_stroke.color,
        ))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(SPACE_16 as i8));
    frame.show(ui, |ui| {
        ui.set_min_width(content_width);
        add_contents(ui);
    });
}

/// 绘制按内容收缩的浮层卡片，避免通知提示被扩展到页面全宽。
pub fn compact_card(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    let frame = Frame::new()
        .fill(ui.visuals().window_fill)
        .stroke(Stroke::new(
            1.0_f32,
            ui.visuals().widgets.inactive.bg_stroke.color,
        ))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(SPACE_16 as i8));
    frame.show(ui, add_contents);
}

pub fn state_card(ui: &mut egui::Ui, title: &str, detail: &str, color: Color32) {
    card(ui, |ui| {
        ui.horizontal(|ui| {
            ui.colored_label(color, "●");
            ui.vertical(|ui| {
                ui.label(RichText::new(title).strong());
                ui.add_space(SPACE_4);
                ui.label(RichText::new(detail).color(ui.visuals().weak_text_color()));
            });
        });
    });
}

pub fn primary_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let button = egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
        .fill(accent(ui))
        .stroke(Stroke::NONE);
    ui.add(button)
}

pub fn danger_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).color(Color32::WHITE).strong())
            .fill(danger())
            .stroke(Stroke::NONE),
    )
}

/// 在固定方形区域内绘制一致的线性工具图标。
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
    let stroke = Stroke::new((size * 0.075).max(1.2), color);
    match icon {
        ToolIcon::FileLock => {
            painter.rect_stroke(
                rect,
                CornerRadius::same(2),
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
                painter.circle_filled(point, size * 0.07, color);
            }
            painter.circle_filled(center, size * 0.09, color);
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
    }
}

#[cfg(test)]
mod tests {
    use super::{first_readable_font, font_candidate_paths};
    use std::path::Path;

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
}
