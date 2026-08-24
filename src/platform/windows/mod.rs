//! Windows 平台服务的统一导出。

mod clock;
mod explorer;
mod file_dialog;
mod network;
mod network_tools;
mod process;
mod restart_manager;
pub mod shell_context_menu;
pub mod single_instance;
pub(crate) mod wide;

pub use clock::local_time_hms;
pub use explorer::{open_directory, open_file_location};
pub use file_dialog::{pick_file, save_csv};
pub use network::query_network_endpoints;
pub use network_tools::{ping_host, ping_host_with_progress, query_dns, tcp_probe};
pub use process::{query_process, terminate_process};
pub use restart_manager::query_file_locks;
