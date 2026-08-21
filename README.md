# Windows Toolbox

Windows Toolbox 是一个面向 Windows 10/11 x64 的原生系统工具集合。V0.1 提供文件占用查询、端口占用查询和进程关系查看，并从一开始采用可扩展的 Tool Host + Tool Module 架构。

## 当前功能

- 文件占用：使用 Restart Manager 查找正在占用文件的进程，支持输入路径、拖放和资源管理器右键调用。
- 端口占用：使用 IP Helper 的扩展 TCP/UDP 表读取 IPv4、IPv6 端点，按端口、进程或 PID 过滤，支持 1 秒、2 秒、5 秒自动刷新。
- 进程关系：显示进程映像路径、启动时间与完整父进程链；端口和文件占用结果可直接跳转到进程详情。
- 资源管理器集成：在设置中按用户选择注册或移除 `HKCU\Software\Classes\*\shell\WindowsToolbox` 下的经典 Shell Verb，不需要管理员权限。Windows 11 上该菜单可能位于“显示更多选项”。
- 单实例：后续启动会通过命名管道将 `ToolInvocation` 转发给已运行实例，并激活对应工具页面。

## 架构

```text
Tool Host (app.rs)
  ├── Tool Registry (src/tools/registry.rs)
  │     ├── File Lock Tool
  │     ├── Port Inspector Tool
  │     └── Process Inspector Tool
  ├── Core: Invocation / Action / Worker
  ├── Domain Model: File / Network / Process / Error
  └── Windows Platform Layer: Restart Manager / IP Helper / Toolhelp / Shell / IPC
```

新增一个内建工具的方式是：在 `src/tools/` 创建模块，实现 `ToolModule`，然后在 `src/tools/mod.rs` 的 `build_registry()` 中注册。导航、搜索和首页会自动发现其描述信息。

## 启动调用

```text
Toolbox.exe --tool file-lock --path "D:\Test\data.db"
Toolbox.exe --tool port-inspector --port 8080
Toolbox.exe --tool process-inspector --pid 18324
```

所有命令行、资源管理器右键菜单和跨工具跳转都会转换为同一个 `ToolInvocation` 结构。

## 构建

目标平台为 `x86_64-pc-windows-msvc`，发布配置启用 LTO、单 codegen unit、静态 CRT、`panic = "abort"` 和 strip。Windows 清单以 `asInvoker` 运行，并声明 Per Monitor V2 DPI awareness。

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release --target x86_64-pc-windows-msvc
```

发布文件预期位置：

```text
target\x86_64-pc-windows-msvc\release\windows-toolbox.exe
```

## 权限与限制

- 默认以普通用户权限运行；部分系统进程的信息或结束操作会因访问限制而失败，并向用户明确提示。
- V0.1 不使用未公开 NT API，因此不读取任意进程命令行，也不实现深度句柄扫描、强制关闭 Handle、驱动、注入或权限绕过。
- Restart Manager 的结果受 Windows 支持范围限制，未返回进程不代表可安全强制解锁。
- 右键菜单采用稳定的传统 Shell Verb；没有实现 Windows 11 一级现代菜单所需的 `IExplorerCommand` COM 扩展。
