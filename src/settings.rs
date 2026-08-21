//! 用户设置的本地 JSON 存储。
//! 仅持久化用户主动选择的外壳状态，不保存系统查询结果或敏感进程信息。

use std::{fs, path::PathBuf};

use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::model::AppError;

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
        let settings_path = directory.join("settings.json");
        Ok(Self {
            directory,
            settings_path,
        })
    }

    pub fn load(&self) -> AppSettings {
        let Ok(contents) = fs::read_to_string(&self.settings_path) else {
            return AppSettings::default();
        };
        serde_json::from_str(&contents).unwrap_or_default()
    }

    pub fn save(&self, settings: &AppSettings) -> Result<(), AppError> {
        let contents = serde_json::to_string_pretty(settings)
            .map_err(|error| AppError::Settings(error.to_string()))?;
        fs::write(&self.settings_path, contents)
            .map_err(|error| AppError::Settings(error.to_string()))
    }

    pub fn eframe_storage_path(&self) -> PathBuf {
        self.directory.join("eframe")
    }
}
