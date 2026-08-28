//! Windows 本地时间格式化服务。

use windows::Win32::System::SystemInformation::GetLocalTime;

/// 返回系统当前本地时间的时分秒，用于标记端口数据刷新时刻。
pub fn local_time_hms() -> String {
    // SAFETY: GetLocalTime 无输入指针，返回按值初始化的 SYSTEMTIME。
    let time = unsafe { GetLocalTime() };
    format!("{:02}:{:02}:{:02}", time.wHour, time.wMinute, time.wSecond)
}

/// 返回系统当前本地时间并保留毫秒，用于通信收发记录。
pub fn local_time_hms_millis() -> String {
    // SAFETY: GetLocalTime 无输入指针，返回按值初始化的 SYSTEMTIME。
    let time = unsafe { GetLocalTime() };
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        time.wHour, time.wMinute, time.wSecond, time.wMilliseconds
    )
}
