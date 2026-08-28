//! 应用外壳与工具页面的响应式布局规则。

/// 顶部全局栏高度，单位为 egui point。
pub(super) const TOP_BAR_HEIGHT: f32 = 50.0;

/// 分类图标栏固定宽度，单位为 egui point。
pub(super) const CATEGORY_RAIL_WIDTH: f32 = 56.0;

/// 当前分类工具栏固定宽度，单位为 egui point。
pub(super) const TOOL_NAV_WIDTH: f32 = 232.0;

/// 分类入口固定高度，图标与交互区域不随文字变化。
pub(super) const CATEGORY_ROW_HEIGHT: f32 = 44.0;

/// 工具入口固定高度，避免字体回退导致导航抖动。
pub(super) const TOOL_ROW_HEIGHT: f32 = 40.0;

/// 窄窗口在该宽度以下收起工具导航栏。
pub(super) const COMPACT_NAV_BREAKPOINT: f32 = 1_120.0;

/// 工具主结果区占可用内容高度的基准比例。
#[cfg(test)]
pub(super) const RESULT_HEIGHT_RATIO: f32 = 0.58;

/// 主内容列的最大宽度，适应宽屏与多列数据表格。
pub(super) const MAX_CONTENT_WIDTH: f32 = 1600.0;

/// 外壳在当前窗口宽度下应使用的导航模式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum NavigationLayout {
    Wide,
    Compact,
}

/// 计算当前导航模式。
pub(super) const fn navigation_layout(window_width: f32) -> NavigationLayout {
    if window_width < COMPACT_NAV_BREAKPOINT {
        NavigationLayout::Compact
    } else {
        NavigationLayout::Wide
    }
}

/// 计算外壳永久占用的横向宽度。
#[cfg(test)]
pub(super) const fn persistent_navigation_width(layout: NavigationLayout) -> f32 {
    match layout {
        NavigationLayout::Wide => CATEGORY_RAIL_WIDTH + TOOL_NAV_WIDTH,
        NavigationLayout::Compact => CATEGORY_RAIL_WIDTH,
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

/// 根据父容器宽度计算内容列宽度。
pub(super) const fn content_width(available_width: f32) -> f32 {
    if available_width < MAX_CONTENT_WIDTH {
        available_width
    } else {
        MAX_CONTENT_WIDTH
    }
}
#[cfg(test)]
mod tests {
    use super::{
        CATEGORY_RAIL_WIDTH, CATEGORY_ROW_HEIGHT, COMPACT_NAV_BREAKPOINT, MAX_CONTENT_WIDTH,
        NavigationLayout, TOOL_NAV_WIDTH, TOOL_ROW_HEIGHT, content_track_width, content_width,
        navigation_layout, persistent_navigation_width, result_height, tool_row_regions,
    };

    #[test]
    fn content_width_is_bounded_without_negative_space() {
        assert_eq!(content_width(2_000.0), MAX_CONTENT_WIDTH);
        assert_eq!(content_width(720.0), 720.0);
        assert_eq!(content_width(0.0), 0.0);
    }

    #[test]
    fn wide_and_compact_navigation_use_stable_tracks() {
        assert_eq!(navigation_layout(980.0), NavigationLayout::Compact);
        assert_eq!(
            navigation_layout(COMPACT_NAV_BREAKPOINT),
            NavigationLayout::Wide
        );
        assert_eq!(
            persistent_navigation_width(NavigationLayout::Wide),
            CATEGORY_RAIL_WIDTH + TOOL_NAV_WIDTH
        );
        assert_eq!(
            persistent_navigation_width(NavigationLayout::Compact),
            CATEGORY_RAIL_WIDTH
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
        assert_eq!(content_track_width(980.0, NavigationLayout::Compact), 924.0);
        assert_eq!(content_track_width(1_200.0, NavigationLayout::Wide), 912.0);
    }

    #[test]
    fn tool_row_regions_do_not_overlap() {
        let (icon, text, badge) = tool_row_regions(TOOL_NAV_WIDTH - 24.0);
        assert!(icon.end <= text.start);
        assert!(text.end <= badge.start);
        assert_eq!(CATEGORY_ROW_HEIGHT, 44.0);
        assert_eq!(TOOL_ROW_HEIGHT, 40.0);
    }
}
