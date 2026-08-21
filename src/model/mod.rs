//! 领域模型与统一错误模型。
//! 平台层负责将 Win32 原始结构转换为本模块的数据类型，UI 不直接接触 FFI 数据。

pub mod error;
pub mod file;
pub mod network;
pub mod process;

pub use error::AppError;
pub use file::FileLockResult;
pub use network::{IpVersion, NetworkEndpoint, NetworkProtocol, TcpState};
pub use process::{ProcessInfo, ProcessSummary};
