//! Tool Registry：集中管理工具描述、搜索索引和工具实例。

use std::{collections::HashSet, time::Instant};

use serde::{Deserialize, Serialize};

use super::ToolModule;

/// 工具的导航分类，分类不是 UI 字符串，避免未来添加本地化时影响业务逻辑。
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum ToolCategory {
    File,
    Network,
    System,
    Developer,
    Text,
    Security,
    Other,
}

impl ToolCategory {
    pub const ALL: [Self; 7] = [
        Self::File,
        Self::Network,
        Self::System,
        Self::Developer,
        Self::Text,
        Self::Security,
        Self::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::File => "文件",
            Self::Network => "网络",
            Self::System => "系统",
            Self::Developer => "开发者",
            Self::Text => "文本",
            Self::Security => "安全",
            Self::Other => "其他",
        }
    }
}

/// 导航、搜索和首页使用的轻量元数据；工具本体由 registry 私有持有。
#[derive(Clone, Debug)]
pub struct ToolDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub category: ToolCategory,
    pub icon: &'static str,
    pub keywords: &'static [&'static str],
}

/// 按注册顺序保存工具，确保导航排序稳定且不依赖 HashMap 的随机遍历顺序。
#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<Box<dyn ToolModule>>,
}

impl ToolRegistry {
    pub fn register(&mut self, tool: Box<dyn ToolModule>) {
        let id = tool.descriptor().id.to_owned();
        self.try_register(tool)
            .unwrap_or_else(|error| panic!("Tool Registry 注册失败：{error}"));
        debug_assert!(self.get(&id).is_some());
    }

    pub fn try_register(&mut self, tool: Box<dyn ToolModule>) -> Result<(), String> {
        let id = tool.descriptor().id;
        if self
            .tools
            .iter()
            .any(|existing| existing.descriptor().id == id)
        {
            return Err(format!("Tool ID 必须唯一：{id}"));
        }
        self.tools.push(tool);
        Ok(())
    }

    pub fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools.iter().map(|tool| tool.descriptor()).collect()
    }

    pub fn categories(&self) -> Vec<ToolCategory> {
        let present: HashSet<_> = self
            .tools
            .iter()
            .map(|tool| tool.descriptor().category)
            .collect();
        ToolCategory::ALL
            .into_iter()
            .filter(|category| present.contains(category))
            .collect()
    }

    pub fn in_category(&self, category: ToolCategory) -> Vec<ToolDescriptor> {
        self.tools
            .iter()
            .map(|tool| tool.descriptor())
            .filter(|descriptor| descriptor.category == category)
            .collect()
    }

    pub fn search(&self, term: &str) -> Vec<ToolDescriptor> {
        let normalized = term.trim().to_lowercase();
        if normalized.is_empty() {
            return self.descriptors();
        }
        self.tools
            .iter()
            .map(|tool| tool.descriptor())
            .filter(|descriptor| {
                descriptor.name.to_lowercase().contains(&normalized)
                    || descriptor.description.to_lowercase().contains(&normalized)
                    || descriptor
                        .keywords
                        .iter()
                        .any(|keyword| keyword.to_lowercase().contains(&normalized))
            })
            .collect()
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut (dyn ToolModule + '_)> {
        self.tools
            .iter_mut()
            .find(|tool| tool.descriptor().id == id)
            .map(Box::as_mut)
    }

    pub fn get(&self, id: &str) -> Option<&(dyn ToolModule + '_)> {
        self.tools
            .iter()
            .find(|tool| tool.descriptor().id == id)
            .map(Box::as_ref)
    }

    pub fn poll_actions(&mut self, now: Instant) -> Vec<crate::core::actions::AppAction> {
        self.tools
            .iter_mut()
            .flat_map(|tool| tool.poll_actions(now))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use eframe::egui;

    use crate::{
        core::{actions::AppAction, invocation::ToolPayload, worker::TaskResult},
        tools::{
            ToolModule, ToolUiContext, build_registry,
            registry::{ToolCategory, ToolDescriptor, ToolRegistry},
        },
    };

    struct StubTool;

    impl ToolModule for StubTool {
        fn descriptor(&self) -> ToolDescriptor {
            ToolDescriptor {
                id: "stub",
                name: "Stub",
                description: "Registry test helper",
                category: ToolCategory::Other,
                icon: "S",
                keywords: &["stub"],
            }
        }

        fn ui(&mut self, _ui: &mut egui::Ui, _context: ToolUiContext) -> Vec<AppAction> {
            Vec::new()
        }

        fn handle_invocation(&mut self, _payload: ToolPayload) -> Vec<AppAction> {
            Vec::new()
        }

        fn handle_task_result(&mut self, _result: TaskResult) {}

        fn set_busy(&mut self, _busy: bool) {}

        fn is_busy(&self) -> bool {
            false
        }

        fn poll_actions(&mut self, _now: Instant) -> Vec<AppAction> {
            Vec::new()
        }
    }

    #[test]
    fn registry_exposes_all_v0_tools() {
        let registry = build_registry();
        assert_eq!(registry.descriptors().len(), 3);
        assert_eq!(registry.categories().len(), 3);
    }

    #[test]
    fn registry_searches_chinese_and_english_keywords() {
        let registry = build_registry();
        assert_eq!(
            registry.search("端口").first().unwrap().id,
            "port-inspector"
        );
        assert_eq!(
            registry.search("listen").first().unwrap().id,
            "port-inspector"
        );
        assert_eq!(registry.search("file").first().unwrap().id, "file-lock");
    }

    #[test]
    fn registry_rejects_duplicate_ids() {
        let mut registry = ToolRegistry::default();
        registry.try_register(Box::new(StubTool)).unwrap();
        assert!(registry.try_register(Box::new(StubTool)).is_err());
    }
}
