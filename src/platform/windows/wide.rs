//! UTF-16 与 Rust 字符串之间的无损转换辅助函数。

use std::{ffi::OsStr, os::windows::ffi::OsStrExt};

/// 将 Windows API 参数编码为 NUL 结尾 UTF-16，调用方必须保持返回缓冲区存活到 API 返回。
pub fn to_wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value
        .as_ref()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 从固定长度或 NUL 结尾 UTF-16 缓冲区恢复 Rust 字符串。
pub fn from_wide(value: &[u16]) -> String {
    let length = value
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(value.len());
    String::from_utf16_lossy(&value[..length])
}
