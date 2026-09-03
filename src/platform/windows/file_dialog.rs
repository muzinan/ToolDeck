//! Windows 原生文件选择器。
//! 该模块仅在 UI 明确请求时打开系统对话框，不缓存或记录用户选择的路径。

use std::fs;
use std::path::PathBuf;

use windows::{
    Win32::{
        Foundation::RPC_E_CHANGED_MODE,
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
            CoTaskMemFree, CoUninitialize,
        },
        UI::Shell::{
            Common::COMDLG_FILTERSPEC, FOS_FORCEFILESYSTEM, FOS_OVERWRITEPROMPT, FileSaveDialog,
            IFileSaveDialog, SIGDN_FILESYSPATH,
        },
    },
    core::PWSTR,
};

use crate::model::AppError;

struct ComApartment(bool);

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: 当前线程由本函数成功调用 CoInitializeEx 初始化，且只在同一线程配对释放。
            unsafe { CoUninitialize() };
        }
    }
}

/// 打开系统保存对话框并将 UTF-8 CSV 写入用户确认的路径；取消时静默返回 `Ok(None)`。
pub fn save_csv(content: &str) -> Result<Option<PathBuf>, AppError> {
    // SAFETY: 当前调用位于 GUI 线程；若 eframe 已用其他模式初始化 COM，则沿用现有 apartment。
    let status = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let apartment = if status.is_ok() {
        ComApartment(true)
    } else if status == RPC_E_CHANGED_MODE {
        ComApartment(false)
    } else {
        let error = windows::core::Error::from_hresult(status);
        return Err(dialog_error("无法初始化文件保存器", &error));
    };
    // SAFETY: FileSaveDialog 是系统注册的进程内 COM 类，接口引用由 windows crate 管理。
    let dialog: IFileSaveDialog = unsafe {
        CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| dialog_error("无法创建文件保存器", &error))?
    };
    let filter_name = crate::platform::windows::wide::to_wide("CSV 文件");
    let filter_spec = crate::platform::windows::wide::to_wide("*.csv");
    let filters = [COMDLG_FILTERSPEC {
        pszName: windows::core::PCWSTR(filter_name.as_ptr()),
        pszSpec: windows::core::PCWSTR(filter_spec.as_ptr()),
    }];
    // SAFETY: filter 缓冲区和对话框在同步调用期间保持有效。
    unsafe { dialog.SetFileTypes(&filters) }
        .map_err(|error| dialog_error("无法配置 CSV 文件类型", &error))?;
    let file_name = crate::platform::windows::wide::to_wide("ports.csv");
    let extension = crate::platform::windows::wide::to_wide("csv");
    // SAFETY: 默认名称与扩展名均为 NUL 结尾 UTF-16，并在对话框显示期间保持有效。
    unsafe { dialog.SetFileName(windows::core::PCWSTR(file_name.as_ptr())) }
        .map_err(|error| dialog_error("无法设置 CSV 默认文件名", &error))?;
    unsafe { dialog.SetDefaultExtension(windows::core::PCWSTR(extension.as_ptr())) }
        .map_err(|error| dialog_error("无法设置 CSV 默认扩展名", &error))?;
    // SAFETY: 只追加文件系统和覆盖确认选项；对话框同步显示。
    let options = unsafe { dialog.GetOptions() }
        .map_err(|error| dialog_error("无法读取文件保存器选项", &error))?;
    unsafe { dialog.SetOptions(options | FOS_FORCEFILESYSTEM | FOS_OVERWRITEPROMPT) }
        .map_err(|error| dialog_error("无法配置文件保存器选项", &error))?;
    // SAFETY: None 表示无显式父 HWND；对话框会同步阻塞当前 GUI 回调。
    if let Err(error) = unsafe { dialog.Show(None) } {
        drop(apartment);
        if error.code().0 as u32 == 0x8007_04C7 {
            return Ok(None);
        }
        return Err(dialog_error("文件保存器无法显示", &error));
    }
    // SAFETY: Show 成功后 GetResult 返回有效 Shell Item，路径由系统提供。
    let item = unsafe { dialog.GetResult() }
        .map_err(|error| dialog_error("无法读取 CSV 保存路径", &error))?;
    // SAFETY: 路径指针由 COM 任务分配器提供，转换后由 pwstr_to_path 释放。
    let raw_path = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }
        .map_err(|error| dialog_error("无法读取 CSV 保存路径", &error))?;
    let path = unsafe { pwstr_to_path(raw_path) };
    fs::write(&path, content.as_bytes())
        .map_err(|error| AppError::Settings(format!("无法写入 CSV 文件：{error}")))?;
    drop(apartment);
    Ok(Some(path))
}

/// 打开系统保存对话框并将 UTF-8 通信记录写入 `.log`；取消时静默返回 `Ok(None)`。
pub fn save_log(content: &str, default_file_name: &str) -> Result<Option<PathBuf>, AppError> {
    // SAFETY: 当前调用位于 GUI 线程；若 eframe 已用其他模式初始化 COM，则沿用现有 apartment。
    let status = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let apartment = if status.is_ok() {
        ComApartment(true)
    } else if status == RPC_E_CHANGED_MODE {
        ComApartment(false)
    } else {
        let error = windows::core::Error::from_hresult(status);
        return Err(dialog_error("无法初始化通信记录保存器", &error));
    };
    // SAFETY: FileSaveDialog 是系统注册的进程内 COM 类，接口引用由 windows crate 管理。
    let dialog: IFileSaveDialog = unsafe {
        CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER)
            .map_err(|error| dialog_error("无法创建通信记录保存器", &error))?
    };
    let filter_name = crate::platform::windows::wide::to_wide("日志文件");
    let filter_spec = crate::platform::windows::wide::to_wide("*.log");
    let filters = [COMDLG_FILTERSPEC {
        pszName: windows::core::PCWSTR(filter_name.as_ptr()),
        pszSpec: windows::core::PCWSTR(filter_spec.as_ptr()),
    }];
    // SAFETY: filter 缓冲区和对话框在同步调用期间保持有效。
    unsafe { dialog.SetFileTypes(&filters) }
        .map_err(|error| dialog_error("无法配置日志文件类型", &error))?;
    let file_name = crate::platform::windows::wide::to_wide(default_file_name);
    let extension = crate::platform::windows::wide::to_wide("log");
    // SAFETY: 默认名称与扩展名均为 NUL 结尾 UTF-16，并在对话框显示期间保持有效。
    unsafe { dialog.SetFileName(windows::core::PCWSTR(file_name.as_ptr())) }
        .map_err(|error| dialog_error("无法设置日志默认文件名", &error))?;
    unsafe { dialog.SetDefaultExtension(windows::core::PCWSTR(extension.as_ptr())) }
        .map_err(|error| dialog_error("无法设置日志默认扩展名", &error))?;
    // SAFETY: 只追加文件系统和覆盖确认选项；对话框同步显示。
    let options = unsafe { dialog.GetOptions() }
        .map_err(|error| dialog_error("无法读取通信记录保存器选项", &error))?;
    unsafe { dialog.SetOptions(options | FOS_FORCEFILESYSTEM | FOS_OVERWRITEPROMPT) }
        .map_err(|error| dialog_error("无法配置通信记录保存器选项", &error))?;
    // SAFETY: None 表示无显式父 HWND；对话框会同步阻塞当前 GUI 回调。
    if let Err(error) = unsafe { dialog.Show(None) } {
        drop(apartment);
        if error.code().0 as u32 == 0x8007_04C7 {
            return Ok(None);
        }
        return Err(dialog_error("通信记录保存器无法显示", &error));
    }
    // SAFETY: Show 成功后 GetResult 返回有效 Shell Item，路径由系统提供。
    let item = unsafe { dialog.GetResult() }
        .map_err(|error| dialog_error("无法读取日志保存路径", &error))?;
    // SAFETY: 路径指针由 COM 任务分配器提供，转换后由 pwstr_to_path 释放。
    let raw_path = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) }
        .map_err(|error| dialog_error("无法读取日志保存路径", &error))?;
    let path = unsafe { pwstr_to_path(raw_path) };
    fs::write(&path, content.as_bytes())
        .map_err(|error| AppError::Settings(format!("无法写入日志文件：{error}")))?;
    drop(apartment);
    Ok(Some(path))
}

unsafe fn pwstr_to_path(value: PWSTR) -> PathBuf {
    // SAFETY: 调用者保证 value 来自 GetDisplayName，指向 NUL 结尾 UTF-16；to_string 仅同步读取。
    let text = unsafe { value.to_string() }.unwrap_or_default();
    // SAFETY: value 由 COM 任务分配器返回，且转换完成后只释放一次。
    unsafe { CoTaskMemFree(Some(value.0.cast())) };
    PathBuf::from(text)
}

fn dialog_error(context: &str, error: &windows::core::Error) -> AppError {
    AppError::WindowsApi {
        context: context.to_owned(),
        code: error.to_string(),
    }
}
