//! 每用户资源管理器右键菜单注册。
//! 仅操作 HKCU\\Software\\Classes，不写入 HKLM，也不要求管理员权限。

use std::{ffi::OsStr, slice};

use windows::{
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR},
        System::Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
            RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegOpenKeyExW, RegSetValueExW,
        },
    },
    core::PCWSTR,
};

use crate::{model::AppError, platform::windows::wide};

const VERB_ROOT: &str = "Software\\Classes\\*\\shell\\WindowsToolbox";
const FILE_LOCK_VERB: &str = "Software\\Classes\\*\\shell\\WindowsToolbox\\shell\\file-lock";
const FILE_LOCK_COMMAND: &str =
    "Software\\Classes\\*\\shell\\WindowsToolbox\\shell\\file-lock\\command";

/// 注册“Windows Toolbox -> 查看文件占用”级联菜单。
pub fn register_context_menu(executable: &OsStr) -> Result<(), AppError> {
    let root = RegistryKey::create(VERB_ROOT)?;
    root.set_default("Windows Toolbox")?;
    root.set_named("MUIVerb", "Windows Toolbox")?;

    let file_lock = RegistryKey::create(FILE_LOCK_VERB)?;
    file_lock.set_default("查看文件占用")?;
    file_lock.set_named("MUIVerb", "查看文件占用")?;

    let command = RegistryKey::create(FILE_LOCK_COMMAND)?;
    command.set_default(&format!(
        "{} --tool file-lock --path \"%1\"",
        quote_windows_argument(executable)
    ))?;
    Ok(())
}

/// 删除本应用唯一拥有的注册表根及其子项，不影响其他右键菜单。
pub fn unregister_context_menu() -> Result<(), AppError> {
    let path = wide::to_wide(VERB_ROOT);
    // Safety: 路径为 NUL 结尾 UTF-16，删除目标被限定为应用专属 HKCU 子树，不会触及 HKLM 或兄弟 Shell Verb。
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr())) };
    if status.0 == 0 || status == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        Err(registry_error(status, "无法移除资源管理器右键菜单"))
    }
}

/// 查询注册表实际状态，而非仅依赖本地 JSON 设置，便于处理用户手动修改注册表的情况。
pub fn is_context_menu_registered() -> bool {
    let path = wide::to_wide(FILE_LOCK_COMMAND);
    let mut key = HKEY::default();
    // Safety: path 是 NUL 结尾 UTF-16，输出 HKEY 地址有效；成功后立即关闭，检查本身不修改注册表。
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(path.as_ptr()),
            None,
            KEY_READ,
            &mut key,
        )
    };
    if status.0 != 0 {
        return false;
    }
    // Safety: key 仅在 RegOpenKeyExW 成功后使用，且此处分支是其唯一关闭点。
    let _ = unsafe { RegCloseKey(key) };
    true
}

struct RegistryKey(HKEY);

impl RegistryKey {
    fn create(path: &str) -> Result<Self, AppError> {
        let wide_path = wide::to_wide(path);
        let mut key = HKEY::default();
        // Safety: 路径是本模块固定的 NUL 结尾 HKCU 子键，key 输出地址在调用结束前有效。
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(wide_path.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_WRITE,
                None,
                &mut key,
                None,
            )
        };
        if status.0 == 0 {
            Ok(Self(key))
        } else {
            Err(registry_error(status, "无法创建右键菜单注册表项"))
        }
    }

    fn set_default(&self, value: &str) -> Result<(), AppError> {
        self.set(PCWSTR::null(), value)
    }

    fn set_named(&self, name: &str, value: &str) -> Result<(), AppError> {
        let wide_name = wide::to_wide(name);
        self.set(PCWSTR(wide_name.as_ptr()), value)
    }

    fn set(&self, name: PCWSTR, value: &str) -> Result<(), AppError> {
        let wide_value = wide::to_wide(value);
        // Safety: UTF-16 缓冲区包含终止符；按字节视图传入 REG_SZ，生命周期覆盖同步注册表调用。
        let bytes = unsafe {
            slice::from_raw_parts(
                wide_value.as_ptr().cast::<u8>(),
                wide_value.len() * std::mem::size_of::<u16>(),
            )
        };
        // Safety: self.0 是当前对象独占的有效 HKEY，name 和 bytes 在同步调用完成前保持有效。
        let status = unsafe { RegSetValueExW(self.0, name, None, REG_SZ, Some(bytes)) };
        if status.0 == 0 {
            Ok(())
        } else {
            Err(registry_error(status, "无法写入右键菜单注册表值"))
        }
    }
}

impl Drop for RegistryKey {
    fn drop(&mut self) {
        // Safety: RegCreateKeyExW 成功后句柄由当前对象唯一持有，析构时关闭不会删除键。
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

/// 使用 Windows 命令行反转义规则包裹可执行文件路径，支持空格、中文、引号和末尾反斜杠。
pub fn quote_windows_argument(argument: &OsStr) -> String {
    let value = argument.to_string_lossy();
    let mut quoted = String::from("\"");
    let mut backslashes = 0_usize;

    for character in value.chars() {
        match character {
            '\\' => backslashes += 1,
            '\"' => {
                quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                quoted.push('\"');
                backslashes = 0;
            }
            _ => {
                quoted.push_str(&"\\".repeat(backslashes));
                quoted.push(character);
                backslashes = 0;
            }
        }
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('\"');
    quoted
}

fn registry_error(status: WIN32_ERROR, context: &str) -> AppError {
    AppError::ShellRegistrationFailed(format!("{context}，Win32 error {}", status.0))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::quote_windows_argument;

    #[test]
    fn quotes_unicode_path_with_spaces() {
        assert_eq!(
            quote_windows_argument(OsStr::new(r"C:\Users\张三\Desktop\工具\Toolbox.exe")),
            r#""C:\Users\张三\Desktop\工具\Toolbox.exe""#
        );
    }

    #[test]
    fn doubles_terminal_backslashes_before_quote() {
        assert_eq!(
            quote_windows_argument(OsStr::new(r"C:\Toolbox\")),
            r#""C:\Toolbox\\""#
        );
    }
}
