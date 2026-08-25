//! 应用布局断点与内容尺寸规则。

/// 主内容列的最大宽度，避免宽屏下信息行被拉得过长。
pub(super) const MAX_CONTENT_WIDTH: f32 = 1040.0;

/// 进程树与详情并排显示的最小可用宽度。
pub(super) const DETAIL_SPLIT_WIDTH: f32 = 760.0;

/// 根据父容器宽度计算内容列宽度。
pub(super) const fn content_width(available_width: f32) -> f32 {
    if available_width < MAX_CONTENT_WIDTH {
        available_width
    } else {
        MAX_CONTENT_WIDTH
    }
}

/// 判断是否有足够空间并排显示列表和详情。
pub(super) const fn use_detail_split(available_width: f32) -> bool {
    available_width >= DETAIL_SPLIT_WIDTH
}

#[cfg(test)]
mod tests {
    use super::{MAX_CONTENT_WIDTH, content_width, use_detail_split};

    #[test]
    fn content_width_is_bounded_without_negative_space() {
        assert_eq!(content_width(1_200.0), MAX_CONTENT_WIDTH);
        assert_eq!(content_width(720.0), 720.0);
        assert_eq!(content_width(0.0), 0.0);
    }

    #[test]
    fn detail_split_uses_the_declared_breakpoint() {
        assert!(!use_detail_split(759.0));
        assert!(use_detail_split(760.0));
    }
}
