# Windows Toolbox

Windows Toolbox 是一个面向 Windows 10/11 x64 的原生系统工具集合。V0.3.0 提供文件、端口、进程、基础网络和 MTR 路径诊断，并从一开始采用可扩展的 Tool Host + Tool Module 架构。

## 当前功能

- 文件占用：使用 Restart Manager 查找正在占用文件的进程，支持系统文件选择器、回车与重查、拖放、完整报告和资源管理器右键调用。
- 端口占用：使用 IP Helper 的扩展 TCP/UDP 表读取 IPv4、IPv6 端点，支持精确端口/PID、协议、IP 版本、完整 TCP 状态筛选、排序、复制行与 CSV，以及 1 秒、2 秒、5 秒自动刷新。
- 进程关系：默认后台加载全部进程树，支持折叠、展开、搜索、刷新、进程选择、WMI 懒加载命令行、完整父进程链和直接子进程。
- 资源管理器集成：在设置中按用户选择注册或移除 `HKCU\Software\Classes\*\shell\WindowsToolbox` 下的经典 Shell Verb，不需要管理员权限。Windows 11 上该菜单可能位于“显示更多选项”。
- 单实例：后续启动会通过命名管道将 `ToolInvocation` 转发给已运行实例，并激活对应工具页面。
- 后台执行：固定四个 Worker 线程与容量 32 的有界请求队列提供背压；同一工具只接收最新请求结果。
- 本地诊断：设置采用原子替换和唯一损坏备份，脱敏诊断日志按 512 KiB、最多三份轮转。

## 架构

```text
Tool Host (src/app/mod.rs)
  ├── Tool Registry (src/tools/registry.rs)
  │     ├── File Lock Tool
  │     ├── Port Inspector Tool
  │     ├── Process Inspector Tool
  │     ├── DNS Lookup Tool
  │     ├── Ping Tool
  │     ├── TCP Probe Tool
  │     └── MTR Tool
  ├── App Shell (src/app/)
  │     ├── state / navigation / content / layout
  │     └── actions / overlay
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
Toolbox.exe --tool dns-lookup --host example.com
Toolbox.exe --tool ping --host example.com
Toolbox.exe --tool tcp-probe --host example.com --port 443
Toolbox.exe --tool mtr --host example.com
```

所有命令行、资源管理器右键菜单和跨工具跳转都会转换为同一个 `ToolInvocation` 结构。

## 构建

目标平台为 `x86_64-pc-windows-msvc`，发布配置启用 LTO、单 codegen unit、静态 CRT、`panic = "abort"` 和 strip。Windows 清单以 `asInvoker` 运行，并声明 Per Monitor V2 DPI awareness。

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo build --release
cargo build --release --target x86_64-pc-windows-msvc
```

发布文件预期位置：

```text
target\x86_64-pc-windows-msvc\release\windows-toolbox.exe
```

## 权限与限制

- 默认以普通用户权限运行；部分系统进程的信息或结束操作会因访问限制而失败，并向用户明确提示。
- V0.3.0 通过公开 Toolhelp、WMI/COM 和 Windows ICMP API 读取当前选中进程的命令行并执行 MTR；不使用未公开 NT API，也不实现深度句柄扫描、强制关闭 Handle、驱动、注入或权限绕过。

网络工具：

- `--tool dns-lookup --host example.com`
- `--tool ping --host example.com`
- `--tool tcp-probe --host example.com --port 443`
- `--tool mtr --host example.com`
- Restart Manager 的结果受 Windows 支持范围限制，未返回进程不代表可安全强制解锁。
- 右键菜单采用稳定的传统 Shell Verb；没有实现 Windows 11 一级现代菜单所需的 `IExplorerCommand` COM 扩展。
