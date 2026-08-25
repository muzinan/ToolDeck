//! Toast 与模态提示的时间策略。

use std::time::Duration;

use super::state::NoticeTone;

/// 返回通知在界面上保持可见的时长。
pub(super) const fn notice_duration(tone: NoticeTone) -> Duration {
    match tone {
        NoticeTone::Success => Duration::from_secs(4),
        NoticeTone::Danger => Duration::from_secs(10),
    }
}
