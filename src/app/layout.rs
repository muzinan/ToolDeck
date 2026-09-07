//! 应用外壳与工具页面的响应式布局规则。

/// 顶部全局栏高度，单位为 egui point。
pub(super) const TOP_BAR_HEIGHT: f32 = 54.0;

/// 宽窗口统一导航栏内容轨道宽度，单位为 egui point。
///
/// 1536pt 审查视口中，290pt 轨道与参考图的侧栏右边界对齐。
pub(super) const UNIFIED_SIDEBAR_WIDTH: f32 = 290.0;

/// 窄窗口收起后的统一导航栏宽度，单位为 egui point。
pub(super) const COMPACT_SIDEBAR_WIDTH: f32 = 72.0;

/// 分类入口固定高度，图标与交互区域不随文字变化。
pub(super) const CATEGORY_ROW_HEIGHT: f32 = 42.0;

/// 首页与主导航入口固定高度，和抽屉分类行与工具行保持一致。
pub(super) const HOME_ROW_HEIGHT: f32 = 38.0;

/// 工具入口固定高度，避免字体回退导致导航抖动。
pub(super) const TOOL_ROW_HEIGHT: f32 = 40.0;

/// 保留该阈值供布局测试覆盖；统一导航不再收起为分类图标列。
#[cfg(test)]
pub(super) const COMPACT_NAV_BREAKPOINT: f32 = 980.0;

/// 工具主结果区占可用内容高度的基准比例。
#[cfg(test)]
pub(super) const RESULT_HEIGHT_RATIO: f32 = 0.58;

/// 主内容列的最大宽度，适应宽屏与多列数据表格。
#[allow(dead_code)]
pub(super) const MAX_CONTENT_WIDTH: f32 = 1600.0;

/// 外壳导航模式。
///
/// 设计稿要求始终保留一列带文字的工具导航，因此当前运行时固定使用宽导航。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NavigationLayout {
    Wide,
    Compact,
}

/// 计算当前导航模式。
pub(super) const fn navigation_layout(_window_width: f32) -> NavigationLayout {
    NavigationLayout::Wide
}

/// 计算外壳永久占用的横向宽度。
pub(super) const fn persistent_navigation_width(layout: NavigationLayout) -> f32 {
    match layout {
        NavigationLayout::Wide => UNIFIED_SIDEBAR_WIDTH,
        NavigationLayout::Compact => COMPACT_SIDEBAR_WIDTH,
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct HorizontalRange {
    start: f32,
    end: f32,
}

#[cfg(test)]
const fn tool_row_regions(width: f32) -> (HorizontalRange, HorizontalRange, HorizontalRange) {
    (
        HorizontalRange {
            start: 8.0,
            end: 34.0,
        },
        HorizontalRange {
            start: 42.0,
            end: width - 42.0,
        },
        HorizontalRange {
            start: width - 34.0,
            end: width - 8.0,
        },
    )
}

#[cfg(test)]
const fn content_track_width(window_width: f32, layout: NavigationLayout) -> f32 {
    window_width - persistent_navigation_width(layout)
}

/// 计算表格、日志和关系树的主结果区高度。
#[cfg(test)]
pub(super) fn result_height(available_height: f32, minimum: f32, maximum: f32) -> f32 {
    (available_height * RESULT_HEIGHT_RATIO).clamp(minimum, maximum)
}

/// 根据父容器宽度计算内容列宽度，充分利用宽屏与多列数据表格。
#[allow(dead_code)]
pub(super) const fn content_width(available_width: f32) -> f32 {
    if available_width > 0.0 {
        available_width
    } else {
        0.0
    }
}

/// 计算应用层界面缩放系数。
///
/// 自动模式固定返回 `1.0`，由 eframe 将该系数与 Windows 原生 DPI 比例组合。
/// 手动模式仅应用用户明确选择的附加缩放，避免按分辨率再次放大导致界面裁切。
pub(super) fn calculate_adaptive_zoom(user_scale: Option<f32>, review_mode: bool) -> f32 {
    if review_mode {
        return 1.0;
    }

    user_scale.unwrap_or(1.0)
}
#[cfg(test)]
mod tests {
    use super::{
        CATEGORY_ROW_HEIGHT, COMPACT_NAV_BREAKPOINT, COMPACT_SIDEBAR_WIDTH, HOME_ROW_HEIGHT,
        NavigationLayout, TOOL_ROW_HEIGHT, UNIFIED_SIDEBAR_WIDTH, calculate_adaptive_zoom,
        content_track_width, content_width, navigation_layout, persistent_navigation_width,
        result_height, tool_row_regions,
    };

    #[test]
    fn content_width_is_bounded_without_negative_space() {
        assert_eq!(content_width(2_000.0), 2_000.0);
        assert_eq!(content_width(720.0), 720.0);
        assert_eq!(content_width(0.0), 0.0);
        assert_eq!(content_width(-10.0), 0.0);
    }

    #[test]
    fn wide_and_compact_navigation_use_stable_tracks() {
        assert_eq!(navigation_layout(979.0), NavigationLayout::Wide);
        assert_eq!(
            navigation_layout(COMPACT_NAV_BREAKPOINT),
            NavigationLayout::Wide
        );
        assert_eq!(
            navigation_layout(COMPACT_NAV_BREAKPOINT + 1.0),
            NavigationLayout::Wide
        );
        assert_eq!(
            persistent_navigation_width(NavigationLayout::Wide),
            UNIFIED_SIDEBAR_WIDTH
        );
        assert_eq!(
            persistent_navigation_width(NavigationLayout::Compact),
            COMPACT_SIDEBAR_WIDTH
        );
    }

    #[test]
    fn main_result_height_keeps_declared_ratio_and_bounds() {
        assert_eq!(result_height(1_000.0, 420.0, 950.0), 580.0);
        assert_eq!(result_height(500.0, 420.0, 950.0), 420.0);
        assert_eq!(result_height(2_000.0, 420.0, 950.0), 950.0);
    }

    #[test]
    fn navigation_preserves_the_minimum_content_track() {
        assert_eq!(content_track_width(980.0, NavigationLayout::Wide), 690.0);
        assert_eq!(content_track_width(1_200.0, NavigationLayout::Wide), 910.0);
    }

    #[test]
    fn tool_row_regions_do_not_overlap() {
        let (icon, text, badge) = tool_row_regions(UNIFIED_SIDEBAR_WIDTH - 24.0);
        assert!(icon.end <= text.start);
        assert!(text.end <= badge.start);
        assert_eq!(CATEGORY_ROW_HEIGHT, 42.0);
        assert_eq!(HOME_ROW_HEIGHT, 38.0);
        assert_eq!(TOOL_ROW_HEIGHT, 40.0);
    }

    #[test]
    fn automatic_zoom_defers_to_native_dpi() {
        assert_eq!(calculate_adaptive_zoom(None, false), 1.0);
        assert_eq!(calculate_adaptive_zoom(Some(1.25), false), 1.25);
        assert_eq!(calculate_adaptive_zoom(Some(1.75), true), 1.0);
    }
}
