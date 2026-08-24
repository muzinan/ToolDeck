//! 工具模块抽象与 V0.1 工具注册。
//! 新工具只需实现 ToolModule 并在 build_registry 注册，即可由导航、搜索和首页自动发现。

mod file_lock;
mod port_inspector;
mod process_inspector;
pub mod registry;

use std::time::Instant;

use eframe::egui;

use crate::core::{actions::AppAction, invocation::ToolPayload, worker::TaskResult};

pub use registry::{ToolDescriptor, ToolIcon, ToolRegistry};

/// 工具 UI 所需的外壳只读状态；禁止将应用状态或其他工具实例直接交给页面。
#[derive(Clone, Copy)]
pub struct ToolUiContext;

/// 编译进主程序的独立工具模块接口。
pub trait ToolModule: Send {
    fn descriptor(&self) -> ToolDescriptor;
    fn ui(&mut self, ui: &mut egui::Ui, context: ToolUiContext) -> Vec<AppAction>;
    fn handle_invocation(&mut self, payload: ToolPayload) -> Vec<AppAction>;
    fn handle_task_result(&mut self, result: TaskResult);
    fn set_busy(&mut self, busy: bool);
    fn is_busy(&self) -> bool;
    fn poll_actions(&mut self, _now: Instant) -> Vec<AppAction> {
        Vec::new()
    }
}

/// V0.1 的内建工具清单。这里是新增模块唯一需要接入外壳的注册位置。
pub fn build_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::default();
    registry.register(Box::new(file_lock::FileLockTool::default()));
    registry.register(Box::new(port_inspector::PortInspectorTool::default()));
    registry.register(Box::new(process_inspector::ProcessInspectorTool::default()));
    registry
}
