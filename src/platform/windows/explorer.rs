//! Windows 资源管理器操作。
//! 仅打开指定文件所在位置，不修改文件或目录内容。

use std::{ffi::OsStr, path::Path};

use windows::{
    Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL},
    core::PCWSTR,
};

use crate::{model::AppError, platform::windows::wide};

/// 在资源管理器中选中一个现有路径。
pub fn open_file_location(path: &Path) -> Result<(), AppError> {
    let executable = wide::to_wide("explorer.exe");
    let parameters_text = format!("/select,\"{}\"", path.display());
    let parameters = wide::to_wide(OsStr::new(&parameters_text));
    // Safety: ShellExecuteW 同步读取 NUL 结尾 UTF-16 参数；两个缓冲区均在调用结束前保持有效。
    let result = unsafe {
        ShellExecuteW(
            None,
            PCWSTR::null(),
            PCWSTR(executable.as_ptr()),
            PCWSTR(parameters.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    let result_code = result.0 as usize;
    if result_code > 32 {
        Ok(())
    } else {
        Err(AppError::WindowsApi {
            context: "无法在资源管理器中打开文件位置".into(),
            code: format!("ShellExecute error {result_code}"),
        })
    }
}
