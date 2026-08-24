//! 用户设置的本地 JSON 存储。
//! 仅持久化用户主动选择的外壳状态，不保存系统查询结果或敏感进程信息。

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use windows::{
    Win32::Storage::FileSystem::{REPLACEFILE_WRITE_THROUGH, ReplaceFileW},
    core::PCWSTR,
};

use crate::{model::AppError, platform::windows::wide};

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "跟随系统",
            Self::Light => "浅色",
            Self::Dark => "深色",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct AppSettings {
    pub theme: ThemePreference,
    pub context_menu_enabled: bool,
    pub recent_tools: Vec<String>,
    pub favorite_tools: Vec<String>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            context_menu_enabled: false,
            recent_tools: Vec::new(),
            favorite_tools: Vec::new(),
        }
    }
}

/// 设置加载结果将可恢复警告与默认回退一起交给 UI，避免损坏配置被静默忽略。
#[derive(Clone, Debug)]
pub struct SettingsLoad {
    pub settings: AppSettings,
    pub warning: Option<String>,
}

/// 设置文件与 eframe 窗口持久化文件的唯一访问入口。
#[derive(Clone, Debug)]
pub struct SettingsStore {
    directory: PathBuf,
    settings_path: PathBuf,
}

impl SettingsStore {
    pub fn open() -> Result<Self, AppError> {
        let dirs = ProjectDirs::from("com", "Toolbox", "WindowsToolbox")
            .ok_or_else(|| AppError::Settings("无法确定当前用户的配置目录。".into()))?;
        let directory = dirs.config_local_dir().to_path_buf();
        fs::create_dir_all(&directory).map_err(|error| AppError::Settings(error.to_string()))?;
        Ok(Self::from_directory(directory))
    }

    fn from_directory(directory: PathBuf) -> Self {
        let settings_path = directory.join("settings.json");
        Self {
            directory,
            settings_path,
        }
    }

    pub fn load(&self) -> SettingsLoad {
        let contents = match fs::read_to_string(&self.settings_path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return SettingsLoad {
                    settings: AppSettings::default(),
                    warning: None,
                };
            }
            Err(error) => {
                return SettingsLoad {
                    settings: AppSettings::default(),
                    warning: Some(format!("无法读取设置，已使用默认值：{error}")),
                };
            }
        };

        match serde_json::from_str(&contents) {
            Ok(settings) => SettingsLoad {
                settings,
                warning: None,
            },
            Err(error) => {
                let backup = self.backup_corrupt_settings();
                let warning = match backup {
                    Ok(path) => format!(
                        "设置文件已损坏，已使用默认值；原文件已备份为 {}：{error}",
                        path.display()
                    ),
                    Err(backup_error) => format!(
                        "设置文件已损坏且无法创建备份，已使用默认值：{error}；备份失败：{backup_error}"
                    ),
                };
                SettingsLoad {
                    settings: AppSettings::default(),
                    warning: Some(warning),
                }
            }
        }
    }

    pub fn save(&self, settings: &AppSettings) -> Result<(), AppError> {
        let contents = serde_json::to_vec_pretty(settings)
            .map_err(|error| AppError::Settings(error.to_string()))?;
        let temporary_path = self.settings_path.with_extension("json.tmp");
        let result = (|| {
            let mut file = fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&temporary_path)
                .map_err(|error| AppError::Settings(error.to_string()))?;
            file.write_all(&contents)
                .and_then(|()| file.sync_all())
                .map_err(|error| AppError::Settings(error.to_string()))?;
            drop(file);
            atomic_replace(&temporary_path, &self.settings_path)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary_path);
        }
        result
    }

    fn backup_corrupt_settings(&self) -> Result<PathBuf, AppError> {
        let backup_path = self.directory.join("settings.corrupt.json");
        let temporary_path = self.directory.join("settings.corrupt.tmp.json");
        fs::copy(&self.settings_path, &temporary_path)
            .map_err(|error| AppError::Settings(error.to_string()))?;
        let temporary = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&temporary_path)
            .map_err(|error| AppError::Settings(error.to_string()))?;
        temporary
            .sync_all()
            .map_err(|error| AppError::Settings(error.to_string()))?;
        drop(temporary);
        if let Err(error) = atomic_replace(&temporary_path, &backup_path) {
            let _ = fs::remove_file(&temporary_path);
            return Err(error);
        }
        // 固定备份已经完整落盘后才移除损坏源文件，避免替换过程中同时失去两份内容。
        fs::remove_file(&self.settings_path)
            .map_err(|error| AppError::Settings(error.to_string()))?;
        Ok(backup_path)
    }

    pub fn eframe_storage_path(&self) -> PathBuf {
        self.directory.join("eframe")
    }
}

fn atomic_replace(temporary_path: &Path, settings_path: &Path) -> Result<(), AppError> {
    if !settings_path.exists() {
        return fs::rename(temporary_path, settings_path)
            .map_err(|error| AppError::Settings(error.to_string()));
    }
    let settings = wide::to_wide(settings_path.as_os_str());
    let temporary = wide::to_wide(temporary_path.as_os_str());
    // SAFETY: 两个路径均为 NUL 结尾 UTF-16；ReplaceFileW 在同目录原子替换目标且不保留额外备份。
    unsafe {
        ReplaceFileW(
            PCWSTR(settings.as_ptr()),
            PCWSTR(temporary.as_ptr()),
            PCWSTR::null(),
            REPLACEFILE_WRITE_THROUGH,
            None,
            None,
        )
    }
    .map_err(|error| AppError::Settings(error.to_string()))
}

#[cfg(test)]
mod tests {
    use std::{fs, time::SystemTime};

    use super::{AppSettings, SettingsStore, ThemePreference};

    fn test_directory(name: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("windows-toolbox-{name}-{stamp}"))
    }

    #[test]
    fn old_settings_json_uses_defaults_for_new_fields() {
        let settings: AppSettings = serde_json::from_str(r#"{"theme":"Dark"}"#).unwrap();
        assert_eq!(settings.theme, ThemePreference::Dark);
        assert!(!settings.context_menu_enabled);
        assert!(settings.favorite_tools.is_empty());
    }

    #[test]
    fn corrupt_settings_keep_one_fixed_backup_and_warn() {
        let directory = test_directory("corrupt-settings");
        fs::create_dir_all(&directory).unwrap();
        let store = SettingsStore::from_directory(directory.clone());
        fs::write(&store.settings_path, "{broken").unwrap();

        let loaded = store.load();

        assert!(loaded.warning.is_some());
        assert!(!store.settings_path.exists());
        assert_eq!(
            fs::read_dir(&directory)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains(".corrupt"))
                .count(),
            1
        );
        fs::write(&store.settings_path, "{broken-again").unwrap();
        let loaded_again = store.load();
        assert!(loaded_again.warning.is_some());
        assert_eq!(
            fs::read_dir(&directory)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains(".corrupt"))
                .count(),
            1
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn settings_save_replaces_existing_json() {
        let directory = test_directory("atomic-save");
        fs::create_dir_all(&directory).unwrap();
        let store = SettingsStore::from_directory(directory.clone());
        store.save(&AppSettings::default()).unwrap();
        let updated = AppSettings {
            theme: ThemePreference::Dark,
            ..AppSettings::default()
        };
        store.save(&updated).unwrap();

        let loaded = store.load();

        assert_eq!(loaded.settings.theme, ThemePreference::Dark);
        assert!(loaded.warning.is_none());
        assert!(!store.settings_path.with_extension("json.tmp").exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
