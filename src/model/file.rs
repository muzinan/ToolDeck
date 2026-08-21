//! 文件占用查询的领域结果。

use serde::{Deserialize, Serialize};

use super::ProcessSummary;

/// Restart Manager 返回的、正在使用指定文件的进程集合。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct FileLockResult {
    /// 由用户输入、拖放或外部调用传入的绝对或相对文件路径。
    pub path: String,
    /// Restart Manager 成功识别到的占用进程。
    pub processes: Vec<ProcessSummary>,
}
