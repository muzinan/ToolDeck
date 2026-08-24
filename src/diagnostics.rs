//! 本地诊断日志的脱敏写入与有界轮转。
//! 启动失败和 tracing 运行事件共用三份日志，不记录文件查询路径、端口或 PID 参数。

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use directories::ProjectDirs;
use tracing_subscriber::fmt::MakeWriter;

const LOG_FILE_NAME: &str = "windows-toolbox.log";
const LOG_FILE_LIMIT: u64 = 512 * 1024;
const LOG_FILE_COUNT: usize = 3;
static LOG_GATE: Mutex<()> = Mutex::new(());
static LOG_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();

#[derive(Clone, Copy, Debug)]
pub enum StartupFailureKind {
    Panic,
    Startup,
}

impl StartupFailureKind {
    fn label(self) -> &'static str {
        match self {
            Self::Panic => "panic",
            Self::Startup => "startup-error",
        }
    }
}

/// tracing 每个事件使用独立缓冲，事件完成时再脱敏并写入共享日志。
#[derive(Clone)]
pub struct DiagnosticMakeWriter {
    directory: PathBuf,
}

pub struct DiagnosticWriter {
    directory: PathBuf,
    buffer: Vec<u8>,
}

impl<'a> MakeWriter<'a> for DiagnosticMakeWriter {
    type Writer = DiagnosticWriter;

    fn make_writer(&'a self) -> Self::Writer {
        DiagnosticWriter {
            directory: self.directory.clone(),
            buffer: Vec::new(),
        }
    }
}

impl Write for DiagnosticWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for DiagnosticWriter {
    fn drop(&mut self) {
        let Ok(_guard) = LOG_GATE.lock() else {
            return;
        };
        let text = String::from_utf8_lossy(&self.buffer);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_secs());
        let record = format!("[{timestamp}] {}\n", sanitize_reason(&text));
        let _ = append_record(&self.directory, &record);
    }
}

/// 构造真正落盘的 tracing writer；LocalAppData 不可用时回退 TEMP。
pub fn tracing_writer() -> Option<DiagnosticMakeWriter> {
    usable_directory().map(|directory| {
        let _ = LOG_DIRECTORY.set(directory.clone());
        DiagnosticMakeWriter { directory }
    })
}

/// 返回当前进程实际选择的诊断目录；尚未初始化时按相同回退策略探测。
pub fn diagnostic_directory() -> Option<PathBuf> {
    LOG_DIRECTORY.get().cloned().or_else(usable_directory)
}

/// 记录经过脱敏的实际失败原因，并返回最终使用的日志路径。
pub fn append_startup_failure(kind: StartupFailureKind, reason: &str) -> Option<PathBuf> {
    let _guard = LOG_GATE.lock().ok()?;
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    let record = format!(
        "[{timestamp}] {}：{}；查询参数与用户路径已脱敏。\n",
        kind.label(),
        sanitize_reason(reason)
    );
    let path = primary_directory()
        .and_then(|directory| append_record(&directory, &record).ok())
        .or_else(|| {
            temporary_directory().and_then(|directory| append_record(&directory, &record).ok())
        });
    if let Some(path) = &path
        && let Some(directory) = path.parent()
    {
        let _ = LOG_DIRECTORY.set(directory.to_path_buf());
    }
    path
}

fn usable_directory() -> Option<PathBuf> {
    primary_directory()
        .filter(|directory| ensure_log_ready(directory).is_ok())
        .or_else(|| temporary_directory().filter(|directory| ensure_log_ready(directory).is_ok()))
}

fn primary_directory() -> Option<PathBuf> {
    ProjectDirs::from("com", "Toolbox", "WindowsToolbox")
        .map(|dirs| dirs.config_local_dir().to_path_buf())
}

fn temporary_directory() -> Option<PathBuf> {
    std::env::var_os("TEMP").map(PathBuf::from)
}

fn ensure_log_ready(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(directory.join(LOG_FILE_NAME))?;
    Ok(())
}

fn append_record(directory: &Path, record: &str) -> io::Result<PathBuf> {
    fs::create_dir_all(directory)?;
    let path = directory.join(LOG_FILE_NAME);
    let current_size = fs::metadata(&path).map_or(0, |metadata| metadata.len());
    if current_size.saturating_add(record.len() as u64) > LOG_FILE_LIMIT {
        rotate_logs(directory)?;
    }
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(record.as_bytes())?;
    file.flush()?;
    Ok(path)
}

fn rotate_logs(directory: &Path) -> io::Result<()> {
    for index in (1..LOG_FILE_COUNT).rev() {
        let source = rotated_path(directory, index - 1);
        let destination = rotated_path(directory, index);
        if destination.exists() {
            fs::remove_file(&destination)?;
        }
        if source.exists() {
            fs::rename(source, destination)?;
        }
    }
    Ok(())
}

fn rotated_path(directory: &Path, index: usize) -> PathBuf {
    if index == 0 {
        directory.join(LOG_FILE_NAME)
    } else {
        directory.join(format!("windows-toolbox.{index}.log"))
    }
}

fn sanitize_reason(reason: &str) -> String {
    let compact = reason.replace(['\r', '\n'], " ");
    let sensitive_markers = [
        "--path",
        "--port",
        "--pid",
        "不支持的启动参数：",
        "实际为：",
    ];
    let cutoff = sensitive_markers
        .iter()
        .filter_map(|marker| compact.find(marker))
        .min()
        .unwrap_or(compact.len());
    let mut safe = compact[..cutoff].trim().to_owned();
    if cutoff < compact.len() {
        safe.push_str("[参数已隐藏]");
    }
    safe = redact_path_tail(&safe);
    safe.chars().take(600).collect()
}

fn redact_path_tail(value: &str) -> String {
    let bytes = value.as_bytes();
    let path_start = bytes.windows(3).position(|window| {
        (window[0].is_ascii_alphabetic() && window[1] == b':' && matches!(window[2], b'\\' | b'/'))
            || (window[0] == b'\\' && window[1] == b'\\')
    });
    path_start.map_or_else(
        || value.to_owned(),
        |index| format!("{}[路径已隐藏]", value[..index].trim_end()),
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{LOG_FILE_COUNT, LOG_FILE_LIMIT, rotated_path, sanitize_reason};

    #[test]
    fn diagnostic_rotation_is_bounded_to_three_files() {
        assert_eq!(LOG_FILE_LIMIT, 512 * 1024);
        assert_eq!(LOG_FILE_COUNT, 3);
        let directory = Path::new("C:/diagnostics");
        assert_eq!(
            rotated_path(directory, 0).file_name().unwrap(),
            "windows-toolbox.log"
        );
        assert_eq!(
            rotated_path(directory, 2).file_name().unwrap(),
            "windows-toolbox.2.log"
        );
    }

    #[test]
    fn diagnostic_reason_redacts_queries_and_paths() {
        assert_eq!(
            sanitize_reason(r"设置失败 C:\Users\张三\secret.json: access denied"),
            "设置失败[路径已隐藏]"
        );
        assert_eq!(
            sanitize_reason("端口错误，实际为：54321"),
            "端口错误，[参数已隐藏]"
        );
    }
}
