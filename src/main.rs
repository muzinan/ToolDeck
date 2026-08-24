//! Windows Toolbox 的应用入口。
//! 该模块只负责初始化日志、解析启动调用、协调单实例 IPC 与创建 GUI 外壳。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod core;
mod model;
mod platform;
mod settings;
mod tools;
mod ui;

use app::ToolboxApp;
use core::invocation::ToolInvocation;
use platform::windows::single_instance::{InstanceRole, SingleInstance};
use settings::SettingsStore;

const APP_NAME: &str = "Windows Toolbox";

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_target(false)
        .without_time()
        .init();

    let initial_invocation = ToolInvocation::from_command_line()?;
    let instance = SingleInstance::acquire(initial_invocation.clone())?;

    let mut primary_instance = match instance {
        InstanceRole::Secondary => return Ok(()),
        InstanceRole::Primary(primary) => primary,
    };
    let receiver = primary_instance.take_receiver();
    // primary_instance 必须保留在 main 的作用域内，确保互斥体句柄直到 run_native 返回后才关闭。

    let settings_store = SettingsStore::open()?;
    let settings = settings_store.load();
    let native_options = app::native_options(&settings, settings_store.eframe_storage_path());

    eframe::run_native(
        APP_NAME,
        native_options,
        Box::new(move |creation_context| {
            Ok(Box::new(ToolboxApp::new(
                creation_context,
                settings_store,
                settings,
                receiver,
                initial_invocation,
            )))
        }),
    )
    .map_err(|error| anyhow::anyhow!("GUI 初始化失败: {error}"))
}
