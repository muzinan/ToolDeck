//! Windows 平台服务的统一导出。

mod explorer;
mod network;
mod process;
mod restart_manager;
pub mod shell_context_menu;
pub mod single_instance;
mod wide;

pub use explorer::open_file_location;
pub use network::query_network_endpoints;
pub use process::{query_process, terminate_process};
pub use restart_manager::query_file_locks;
