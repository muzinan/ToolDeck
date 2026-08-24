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
use std::{
    fs, panic,
    path::PathBuf,
    process::ExitCode,
    time::{SystemTime, UNIX_EPOCH},
};
use windows::{
    Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW},
    core::PCWSTR,
};

const APP_NAME: &str = "Windows Toolbox";

fn main() -> ExitCode {
    panic::set_hook(Box::new(|info| {
        report_startup_failure(&format!("程序发生未处理异常：{info}"))
    }));
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            report_startup_failure(&format!("Windows Toolbox 无法启动：{error}"));
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
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

/// 启动阶段失败优先写入用户本地诊断文件，目录不可用时回退 TEMP；两者失败仍显示消息框。
fn report_startup_failure(message: &str) {
    let primary = directories::ProjectDirs::from("com", "Toolbox", "WindowsToolbox")
        .map(|dirs| dirs.config_local_dir().to_path_buf());
    let diagnostic_path = primary
        .and_then(|directory| append_diagnostic(&directory, message))
        .or_else(|| {
            std::env::var_os("TEMP")
                .and_then(|directory| append_diagnostic(&PathBuf::from(directory), message))
        });
    let message = diagnostic_path.map_or_else(
        || format!("{message}\n诊断日志写入失败。"),
        |path| format!("{message}\n诊断日志：{}", path.display()),
    );
    let title = platform::windows::wide::to_wide(APP_NAME);
    let text = platform::windows::wide::to_wide(&message);
    // SAFETY: 两个 UTF-16 缓冲区在同步 MessageBoxW 返回前保持有效且均 NUL 结尾。
    let _ = unsafe {
        MessageBoxW(
            None,
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        )
    };
}

/// 诊断文件采用追加写入，目录或文件不可写时返回 None 让调用方尝试 TEMP 回退。
fn append_diagnostic(directory: &PathBuf, message: &str) -> Option<PathBuf> {
    fs::create_dir_all(directory).ok()?;
    let path = directory.join("startup-diagnostic.txt");
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    writeln!(file, "[{timestamp}] {message}").ok()?;
    Some(path)
}
