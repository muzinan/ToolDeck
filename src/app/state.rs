//! 应用外壳的轻量状态类型；业务工具状态仍由各自 ToolModule 持有。

use std::time::Instant;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Page {
    Home,
    Tool(String),
    Settings,
    About,
}
/// 主页展示模式：科技网格卡片 vs 紧凑列表（支持海量工具快速扫描）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum HomeViewMode {
    #[default]
    Grid,
    List,
}

impl HomeViewMode {
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Self::Grid => "网格卡片",
            Self::List => "紧凑列表",
        }
    }
}

/// 轻量级成功或失败提示；错误细节会留在对应工具页，外壳提示只说明刚发生的操作结果。
#[derive(Clone, Debug)]
pub(super) struct Notice {
    pub(super) message: String,
    pub(super) tone: NoticeTone,
    pub(super) expires_at: Instant,
}

/// 通知的语义类别在绘制时映射到当前主题颜色，主题切换后不保留旧色值。
#[derive(Clone, Copy, Debug)]
pub(super) enum NoticeTone {
    Success,
    Danger,
}
