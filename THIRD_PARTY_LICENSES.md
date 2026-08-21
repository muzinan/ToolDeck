# Third-Party Licenses

Windows Toolbox 的直接编译依赖均采用宽松许可证；下表版本与当前 `Cargo.lock` 一致。发布前仍应对锁定的全部传递依赖执行一次法律与许可证复核。

| Dependency | Locked version | License | Purpose |
| --- | --- | --- | --- |
| anyhow | 1.0.104 | MIT OR Apache-2.0 | 应用入口错误传播 |
| directories | 6.0.0 | MIT OR Apache-2.0 | Windows 用户配置目录 |
| eframe / egui | 0.32.3 | MIT OR Apache-2.0 | 原生桌面 GUI |
| serde / serde_json | 1.0.229 / 1.0.151 | MIT OR Apache-2.0 | Invocation 与设置序列化 |
| thiserror | 2.0.20 | MIT OR Apache-2.0 | 统一错误模型 |
| tracing / tracing-subscriber | 0.1.44 / 0.3.23 | MIT | 结构化诊断日志 |
| windows | 0.61.3 | MIT OR Apache-2.0 | Win32 API 绑定 |
| embed-resource | 2.5.2 | MIT | 嵌入 Windows 清单资源 |

项目未有意引入 GPL、AGPL 或其他可能限制商业闭源分发的直接依赖。
