//! 工具模块抽象与当前版本工具注册。
//! 新工具只需实现 ToolModule 并在 build_registry 注册，即可由导航、搜索和首页自动发现。

mod communication_tools;
mod file_lock;
mod mtr;
mod network_tools;
mod port_inspector;
mod process_inspector;
pub mod registry;

use std::time::Instant;

use eframe::egui;

use crate::{
    core::{actions::AppAction, invocation::ToolPayload, worker::TaskResult},
    model::{CommunicationEventEnvelope, MtrProgress, PingProgress, TcpProbeProgress},
};

pub use registry::{ToolCategory, ToolDescriptor, ToolIcon, ToolRegistry};

/// UI 审查入口使用的固定页面模式；正式运行始终使用 Default。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UiReviewVariant {
    #[default]
    Default,
    TcpServer,
    UdpSpecial,
}

/// 工具 UI 所需的外壳只读状态；禁止将应用状态或其他工具实例直接交给页面。
#[derive(Clone, Copy, Default)]
pub struct ToolUiContext {
    pub review_mode: bool,
    pub review_variant: UiReviewVariant,
}

/// 编译进主程序的独立工具模块接口。
pub trait ToolModule: Send {
    fn descriptor(&self) -> ToolDescriptor;
    fn ui(&mut self, ui: &mut egui::Ui, context: ToolUiContext) -> Vec<AppAction>;
    fn handle_invocation(&mut self, payload: ToolPayload) -> Vec<AppAction>;
    fn handle_task_result(&mut self, result: TaskResult);
    /// 接收长任务的非终态进度；短任务和不需要进度的工具使用默认空实现。
    fn handle_task_progress(
        &mut self,
        _message: String,
        _completed: Option<u64>,
        _total: Option<u64>,
    ) {
    }
    fn handle_mtr_progress(&mut self, _progress: MtrProgress) {}
    fn handle_ping_progress(&mut self, _progress: PingProgress) {}
    fn handle_tcp_probe_progress(&mut self, _progress: TcpProbeProgress) {}
    /// 接收持久通信会话事件；非通信工具使用默认空实现。
    fn handle_communication_event(&mut self, _event: CommunicationEventEnvelope) {}
    fn set_busy(&mut self, busy: bool);
    fn poll_actions(&mut self, _now: Instant) -> Vec<AppAction> {
        Vec::new()
    }
}

/// V0.4.2 的内建工具清单。这里是新增模块唯一需要接入外壳的注册位置。
pub fn build_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::default();
    registry.register(Box::new(file_lock::FileLockTool::default()));
    registry.register(Box::new(port_inspector::PortInspectorTool::default()));
    registry.register(Box::new(process_inspector::ProcessInspectorTool::default()));
    registry.register(Box::new(network_tools::DnsLookupTool::default()));
    registry.register(Box::new(network_tools::PingTool::default()));
    registry.register(Box::new(network_tools::TcpProbeTool::default()));
    registry.register(Box::new(mtr::MtrTool::default()));
    registry.register(Box::new(communication_tools::TcpDebugTool::default()));
    registry.register(Box::new(communication_tools::UdpDebugTool::default()));
    registry.register(Box::new(communication_tools::SerialDebugTool::default()));
    registry
}
