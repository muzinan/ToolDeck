//! 通过 Restart Manager 查询文件占用进程。
//! V0.1 仅使用受支持的 Rm* API，不依赖未公开的系统句柄枚举接口。

use std::path::Path;

use windows::{
    Win32::{
        Foundation::{ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, WIN32_ERROR},
        System::RestartManager::{
            CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmEndSession, RmGetList, RmRegisterResources,
            RmStartSession,
        },
    },
    core::{PCWSTR, PWSTR},
};

use crate::{
    model::{AppError, FileLockResult, ProcessSummary},
    platform::windows::{process::query_process_summary, wide},
};

/// 在函数生命周期内拥有 Restart Manager 会话，确保任何提前返回都调用 RmEndSession。
struct RestartManagerSession(u32);

impl RestartManagerSession {
    fn start() -> Result<Self, AppError> {
        let mut handle = 0_u32;
        let mut session_key = vec![0_u16; CCH_RM_SESSION_KEY as usize + 1];

        // Safety: 缓冲区长度符合 CCH_RM_SESSION_KEY 要求，并在调用期间保持可写且有效。
        let status = unsafe { RmStartSession(&mut handle, None, PWSTR(session_key.as_mut_ptr())) };
        status_ok(status, "无法启动 Restart Manager 会话")?;
        Ok(Self(handle))
    }
}

impl Drop for RestartManagerSession {
    fn drop(&mut self) {
        // Safety: 会话句柄仅在 RmStartSession 成功后构造，析构时尚未移动，RmEndSession 至多调用一次。
        let _ = unsafe { RmEndSession(self.0) };
    }
}

/// 查询给定文件正在被哪些进程占用。
pub fn query_file_locks(path: &Path) -> Result<FileLockResult, AppError> {
    let canonical_path = std::fs::canonicalize(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => AppError::NotFound(path.display().to_string()),
        std::io::ErrorKind::PermissionDenied => AppError::AccessDenied(path.display().to_string()),
        _ => AppError::InvalidPath(format!("{} ({error})", path.display())),
    })?;
    let metadata = std::fs::metadata(&canonical_path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => AppError::NotFound(canonical_path.display().to_string()),
        std::io::ErrorKind::PermissionDenied => {
            AppError::AccessDenied(canonical_path.display().to_string())
        }
        _ => AppError::InvalidPath(format!("{} ({error})", canonical_path.display())),
    })?;
    if !metadata.is_file() {
        return Err(AppError::InvalidPath(format!(
            "{} 不是普通文件。",
            canonical_path.display()
        )));
    }

    let session = RestartManagerSession::start()?;
    let wide_path = wide::to_wide(canonical_path.as_os_str());
    let paths = [PCWSTR(wide_path.as_ptr())];

    // Safety: paths 仅包含一个有效且 NUL 结尾的 UTF-16 文件路径，调用同步完成后才会释放。
    let status = unsafe { RmRegisterResources(session.0, Some(&paths), None, None) };
    status_ok(status, "无法向 Restart Manager 注册文件")?;

    let affected = read_affected_processes(session.0)?;
    let processes = affected
        .into_iter()
        .map(|entry| {
            let pid = entry.Process.dwProcessId;
            // 进程在快照与 Restart Manager 之间退出时，退回 RM 提供的应用名，其余字段按未知处理。
            query_process_summary(pid).unwrap_or_else(|_| ProcessSummary {
                pid,
                name: wide::from_wide(&entry.strAppName),
                exe_path: None,
                owner: None,
                started_at: None,
                run_state: crate::model::ProcessRunState::Unknown,
            })
        })
        .collect();

    Ok(FileLockResult {
        path: canonical_path.display().to_string(),
        processes,
    })
}

fn read_affected_processes(session: u32) -> Result<Vec<RM_PROCESS_INFO>, AppError> {
    for _ in 0..3 {
        let mut needed = 0_u32;
        let mut available = 0_u32;
        let mut reboot_reasons = 0_u32;

        // Safety: 首次调用只获取当前所需缓冲区大小，因此 affected apps 指针为空且所有计数指针均有效可写。
        let status = unsafe {
            RmGetList(
                session,
                &mut needed,
                &mut available,
                None,
                &mut reboot_reasons,
            )
        };
        if status.0 == 0 && needed == 0 {
            return Ok(Vec::new());
        }
        if status != ERROR_MORE_DATA && status.0 != 0 {
            return Err(status_error(status, "无法读取 Restart Manager 进程列表"));
        }

        let mut entries = vec![RM_PROCESS_INFO::default(); needed as usize];
        available = needed;
        // Safety: entries 分配了 needed 个完整结构体；Restart Manager 最多写入 available 项，所有输出指针有效。
        let status = unsafe {
            RmGetList(
                session,
                &mut needed,
                &mut available,
                Some(entries.as_mut_ptr()),
                &mut reboot_reasons,
            )
        };
        if status.0 == 0 {
            entries.truncate(available as usize);
            return Ok(entries);
        }
        if status != ERROR_MORE_DATA {
            return Err(status_error(status, "无法读取 Restart Manager 进程列表"));
        }
        // 查询窗口内有进程变动时再次按新容量读取，避免截断刚出现的占用者。
    }

    Err(AppError::WindowsApi {
        context: "文件占用进程在查询期间持续变化".into(),
        code: "Restart Manager 返回 ERROR_MORE_DATA".into(),
    })
}

fn status_ok(status: WIN32_ERROR, context: &str) -> Result<(), AppError> {
    if status.0 == 0 {
        Ok(())
    } else {
        Err(status_error(status, context))
    }
}

fn status_error(status: WIN32_ERROR, context: &str) -> AppError {
    match status {
        ERROR_ACCESS_DENIED => AppError::AccessDenied(context.into()),
        ERROR_FILE_NOT_FOUND => AppError::NotFound(context.into()),
        _ => AppError::WindowsApi {
            context: context.to_owned(),
            code: format!("Win32 error {}", status.0),
        },
    }
}
