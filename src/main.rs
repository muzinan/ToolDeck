//! Windows Toolbox 的应用入口。
//! 该模块只负责初始化日志、解析启动调用、协调单实例 IPC 与创建 GUI 外壳。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod core;
mod diagnostics;
mod model;
mod platform;
mod settings;
mod tools;
mod ui;

use app::{ToolboxApp, UiReviewOptions};
use core::invocation::ToolInvocation;
use platform::windows::single_instance::{InstanceRole, SingleInstance};
use settings::{AppSettings, SettingsLoad, SettingsStore};
use std::{panic, process::ExitCode};
use windows::{
    Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW},
    core::PCWSTR,
};

const APP_NAME: &str = "Windows Toolbox";

fn main() -> ExitCode {
    panic::set_hook(Box::new(|info| {
        report_startup_failure(
            &format!("程序发生未处理异常：{info}"),
            diagnostics::StartupFailureKind::Panic,
        )
    }));
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            report_startup_failure(
                &format!("Windows Toolbox 无法启动：{error}"),
                diagnostics::StartupFailureKind::Startup,
            );
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    if let Some(writer) = diagnostics::tracing_writer() {
        tracing_subscriber::fmt()
            .with_env_filter(runtime_log_filter())
            .with_target(false)
            .without_time()
            .with_writer(writer)
            .try_init()
            .map_err(|error| anyhow::anyhow!("诊断日志初始化失败：{error}"))?;
    } else {
        tracing_subscriber::fmt()
            .with_env_filter(runtime_log_filter())
            .with_target(false)
            .without_time()
            .with_writer(std::io::sink)
            .try_init()
            .map_err(|error| anyhow::anyhow!("诊断日志初始化失败：{error}"))?;
    }
    tracing::info!("应用启动");

    #[cfg(feature = "ui-review")]
    let review = ui_review_request()?;
    #[cfg(not(feature = "ui-review"))]
    let review: Option<(String, std::path::PathBuf, [f32; 2])> = None;
    let initial_invocation = if review.is_some() {
        None
    } else {
        ToolInvocation::from_command_line()?
    };
    let instance = SingleInstance::acquire(initial_invocation.clone())?;

    let mut primary_instance = match instance {
        InstanceRole::Secondary => return Ok(()),
        InstanceRole::Primary(primary) => primary,
    };
    tracing::info!("单实例主进程已就绪");
    let receiver = primary_instance.take_receiver();
    // primary_instance 必须保留在 main 的作用域内，确保互斥体句柄直到 run_native 返回后才关闭。

    let settings_store = SettingsStore::open()?;
    let loaded_settings = if review.is_some() {
        SettingsLoad {
            settings: AppSettings::default(),
            warning: None,
        }
    } else {
        settings_store.load()
    };
    if loaded_settings.warning.is_some() {
        tracing::warn!("设置加载失败，已回退默认值并保留唯一损坏备份");
    }
    let settings = loaded_settings.settings;
    let mut native_options = app::native_options(&settings, settings_store.eframe_storage_path());
    if let Some((_, _, size)) = &review {
        native_options.viewport = eframe::egui::ViewportBuilder::default()
            .with_inner_size(*size)
            .with_min_inner_size([980.0, 640.0])
            .with_maximized(false)
            .with_fullscreen(false)
            .with_resizable(false);
        native_options.persist_window = false;
    }
    let review_page = review.as_ref().map(|(page, _, _)| page.clone());
    let review_width = review.as_ref().map(|(_, _, size)| size[0]);
    #[cfg(feature = "ui-review")]
    let review_height = review.as_ref().map(|(_, _, size)| size[1]);

    eframe::run_native(
        APP_NAME,
        native_options,
        Box::new(move |creation_context| {
            Ok(Box::new(ToolboxApp::new(
                creation_context,
                settings_store,
                settings,
                loaded_settings.warning,
                receiver,
                initial_invocation,
                UiReviewOptions {
                    page: review_page,
                    width: review_width,
                    #[cfg(feature = "ui-review")]
                    height: review_height,
                    #[cfg(feature = "ui-review")]
                    screenshot_path: review.as_ref().map(|(_, path, _)| path.clone()),
                },
            )?))
        }),
    )
    .map_err(|error| anyhow::anyhow!("GUI 初始化失败: {error}"))
}

#[cfg(feature = "ui-review")]
fn ui_review_request() -> anyhow::Result<Option<(String, std::path::PathBuf, [f32; 2])>> {
    let mut arguments = std::env::args_os().skip(1);
    let Some(first) = arguments.next() else {
        return Ok(None);
    };
    if first != "--ui-review" {
        return Ok(None);
    }
    let page = arguments
        .next()
        .map(|value| value.to_string_lossy().into_owned())
        .ok_or_else(|| anyhow::anyhow!("--ui-review 缺少页面 ID"))?;
    let screenshot_flag = arguments
        .next()
        .ok_or_else(|| anyhow::anyhow!("--ui-review 缺少 --screenshot"))?;
    if screenshot_flag != "--screenshot" {
        anyhow::bail!("--ui-review 页面后必须指定 --screenshot");
    }
    let path = arguments
        .next()
        .map(std::path::PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("--screenshot 缺少 PNG 路径"))?;
    let size = match arguments.next() {
        Some(flag) if flag == "--size" => {
            let value = arguments
                .next()
                .map(|value| value.to_string_lossy().into_owned())
                .ok_or_else(|| anyhow::anyhow!("--size 缺少 WIDTHxHEIGHT"))?;
            parse_ui_review_size(&value)?
        }
        Some(_) => anyhow::bail!("--ui-review 只接受可选的 --size WIDTHxHEIGHT"),
        None => [1536.0, 1024.0],
    };
    if arguments.next().is_some() {
        anyhow::bail!("--ui-review 不接受额外参数");
    }
    let valid = [
        "home",
        "file-lock",
        "port-inspector",
        "process-inspector",
        "dns-lookup",
        "ping",
        "tcp-probe",
        "mtr",
        "tcp-debug",
        "tcp-debug-server",
        "udp-debug",
        "udp-debug-special",
        "serial-debug",
        "settings",
        "about",
    ];
    if !valid.contains(&page.as_str()) {
        anyhow::bail!("未知的 UI 审查页面：{page}");
    }
    if path.extension().and_then(|value| value.to_str()) != Some("png") {
        anyhow::bail!("UI 审查截图必须使用 .png 扩展名");
    }
    Ok(Some((page, path, size)))
}

#[cfg(feature = "ui-review")]
fn parse_ui_review_size(value: &str) -> anyhow::Result<[f32; 2]> {
    let (width, height) = value
        .split_once('x')
        .ok_or_else(|| anyhow::anyhow!("UI 审查尺寸必须使用 WIDTHxHEIGHT"))?;
    let width = width
        .parse::<u32>()
        .map_err(|_| anyhow::anyhow!("UI 审查宽度必须是整数"))?;
    let height = height
        .parse::<u32>()
        .map_err(|_| anyhow::anyhow!("UI 审查高度必须是整数"))?;
    if !(980..=3840).contains(&width) || !(640..=2160).contains(&height) {
        anyhow::bail!("UI 审查尺寸必须位于 980×640 至 3840×2160 之间");
    }
    Ok([width as f32, height as f32])
}

fn runtime_log_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
}

/// 启动阶段失败优先写入用户本地诊断文件，目录不可用时回退 TEMP；两者失败仍显示消息框。
fn report_startup_failure(message: &str, kind: diagnostics::StartupFailureKind) {
    let diagnostic_path = diagnostics::append_startup_failure(kind, message);
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
