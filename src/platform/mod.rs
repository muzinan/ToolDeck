//! 平台适配层。
//! 当前版本明确以 Windows 为目标，Windows FFI 代码集中在该模块以隔离 `unsafe` 边界。

pub mod windows;
