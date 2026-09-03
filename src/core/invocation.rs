//! 命令行、资源管理器右键菜单和内部跳转共享的工具调用协议。

use std::{ffi::OsString, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::AppError;

/// 仅用于新实例唤醒主窗口的保留工具 ID，不暴露为命令行或工具注册表项。
const ACTIVATE_TOOL_ID: &str = "__windows_toolbox_activate__";

/// 发送到工具模块的结构化输入，不允许调用方直接篡改工具 UI 状态。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct ToolInvocation {
    pub tool_id: String,
    pub payload: ToolPayload,
}

/// 当前工具可接收的输入；新增工具可扩展此枚举而不改变导航和 IPC 协议。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub enum ToolPayload {
    None,
    FilePath { path: PathBuf },
    Port { port: u16 },
    Process { pid: u32 },
    Host { host: String },
    HostPort { host: String, port: u16 },
}

impl ToolInvocation {
    /// 创建仅用于单实例 IPC 的窗口激活消息，序列化格式仍是兼容的 ToolInvocation JSON。
    pub fn activate() -> Self {
        Self {
            tool_id: ACTIVATE_TOOL_ID.to_owned(),
            payload: ToolPayload::None,
        }
    }

    /// 判断消息是否仅请求恢复并聚焦主窗口，不应触发工具导航或错误提示。
    pub fn is_activate(&self) -> bool {
        self.tool_id == ACTIVATE_TOOL_ID && self.payload == ToolPayload::None
    }

    pub fn file_lock(path: PathBuf) -> Self {
        Self {
            tool_id: "file-lock".to_owned(),
            payload: ToolPayload::FilePath { path },
        }
    }

    pub fn process(pid: u32) -> Self {
        Self {
            tool_id: "process-inspector".to_owned(),
            payload: ToolPayload::Process { pid },
        }
    }

    pub fn from_command_line() -> Result<Option<Self>, AppError> {
        Self::parse(std::env::args_os().skip(1))
    }

    pub fn parse<I>(args: I) -> Result<Option<Self>, AppError>
    where
        I: IntoIterator<Item = OsString>,
    {
        let mut tool_id = None;
        let mut path = None;
        let mut port = None;
        let mut pid = None;
        let mut host = None;
        let mut values = args.into_iter();

        while let Some(argument) = values.next() {
            match argument.to_string_lossy().as_ref() {
                "--tool" => {
                    tool_id = Some(next_value(&mut values, "--tool")?);
                }
                "--path" => {
                    path = Some(PathBuf::from(next_value(&mut values, "--path")?));
                }
                "--port" => {
                    let raw = next_value(&mut values, "--port")?;
                    let parsed = raw.parse::<u16>().map_err(|_| {
                        AppError::InvalidInput(format!(
                            "端口必须是 1 到 65535 的整数，实际为：{raw}"
                        ))
                    })?;
                    if parsed == 0 {
                        return Err(AppError::InvalidInput("端口必须大于 0。".into()));
                    }
                    port = Some(parsed);
                }
                "--pid" => {
                    let raw = next_value(&mut values, "--pid")?;
                    let parsed = raw.parse::<u32>().map_err(|_| {
                        AppError::InvalidInput(format!("PID 必须是正整数，实际为：{raw}"))
                    })?;
                    if parsed == 0 {
                        return Err(AppError::InvalidInput("PID 必须大于 0。".into()));
                    }
                    pid = Some(parsed);
                }
                "--host" => {
                    host = Some(next_value(&mut values, "--host")?);
                }
                unknown => {
                    return Err(AppError::InvalidInput(format!(
                        "不支持的启动参数：{unknown}"
                    )));
                }
            }
        }

        let Some(tool_id) = tool_id else {
            if path.is_some() || port.is_some() || pid.is_some() || host.is_some() {
                return Err(AppError::InvalidInput(
                    "使用 --path、--port、--pid 或 --host 时必须指定 --tool。".into(),
                ));
            }
            return Ok(None);
        };

        let payload = match tool_id.as_str() {
            "file-lock" => ToolPayload::FilePath {
                path: path
                    .ok_or_else(|| AppError::InvalidInput("file-lock 需要 --path。".into()))?,
            },
            "port-inspector" => ToolPayload::Port {
                port: port
                    .ok_or_else(|| AppError::InvalidInput("port-inspector 需要 --port。".into()))?,
            },
            "process-inspector" | "process" => ToolPayload::Process {
                pid: pid.ok_or_else(|| {
                    AppError::InvalidInput("process-inspector 需要 --pid。".into())
                })?,
            },
            "dns-lookup" | "ping" | "mtr" => ToolPayload::Host {
                host: host
                    .ok_or_else(|| AppError::InvalidInput(format!("{tool_id} 需要 --host。")))?,
            },
            "tcp-probe" => ToolPayload::HostPort {
                host: host
                    .ok_or_else(|| AppError::InvalidInput("tcp-probe 需要 --host。".into()))?,
                port: port
                    .ok_or_else(|| AppError::InvalidInput("tcp-probe 需要 --port。".into()))?,
            },
            _ => {
                return Err(AppError::InvalidInput(format!("未知工具 ID：{tool_id}")));
            }
        };

        let normalized_tool_id = if tool_id == "process" {
            "process-inspector".to_owned()
        } else {
            tool_id
        };
        Ok(Some(Self {
            tool_id: normalized_tool_id,
            payload,
        }))
    }
}

fn next_value<I>(values: &mut I, flag: &str) -> Result<String, AppError>
where
    I: Iterator<Item = OsString>,
{
    values
        .next()
        .map(|value| value.to_string_lossy().into_owned())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::InvalidInput(format!("{flag} 缺少参数值。")))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::{ToolInvocation, ToolPayload};

    #[test]
    fn parses_file_lock_with_unicode_path() {
        let invocation = ToolInvocation::parse([
            OsString::from("--tool"),
            OsString::from("file-lock"),
            OsString::from("--path"),
            OsString::from(r"C:\\Users\\张三\\桌面\\我的 项目\\data.db"),
        ])
        .unwrap()
        .unwrap();

        assert_eq!(invocation.tool_id, "file-lock");
        assert!(matches!(invocation.payload, ToolPayload::FilePath { .. }));
    }

    #[test]
    fn rejects_missing_required_payload() {
        assert!(
            ToolInvocation::parse([OsString::from("--tool"), OsString::from("file-lock")]).is_err()
        );
    }

    #[test]
    fn normalizes_process_alias() {
        let invocation = ToolInvocation::parse([
            OsString::from("--tool"),
            OsString::from("process"),
            OsString::from("--pid"),
            OsString::from("42"),
        ])
        .unwrap()
        .unwrap();

        assert_eq!(invocation.tool_id, "process-inspector");
    }

    #[test]
    fn rejects_zero_port_and_pid() {
        assert!(
            ToolInvocation::parse([
                OsString::from("--tool"),
                OsString::from("port-inspector"),
                OsString::from("--port"),
                OsString::from("0"),
            ])
            .is_err()
        );
        assert!(
            ToolInvocation::parse([
                OsString::from("--tool"),
                OsString::from("process-inspector"),
                OsString::from("--pid"),
                OsString::from("0"),
            ])
            .is_err()
        );
    }

    #[test]
    fn activate_message_is_internal_and_round_trips_through_json() {
        let invocation = ToolInvocation::activate();
        assert!(invocation.is_activate());

        let encoded = serde_json::to_string(&invocation).unwrap();
        let decoded: ToolInvocation = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, invocation);
        assert!(decoded.is_activate());
    }

    #[test]
    fn parse_without_arguments_keeps_existing_none_semantics() {
        assert_eq!(ToolInvocation::parse(Vec::<OsString>::new()).unwrap(), None);
    }

    #[test]
    fn parses_network_tool_invocations() {
        let dns = ToolInvocation::parse([
            OsString::from("--tool"),
            OsString::from("dns-lookup"),
            OsString::from("--host"),
            OsString::from("example.com"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(dns.tool_id, "dns-lookup");
        assert!(matches!(dns.payload, ToolPayload::Host { .. }));
        let tcp = ToolInvocation::parse([
            OsString::from("--tool"),
            OsString::from("tcp-probe"),
            OsString::from("--host"),
            OsString::from("localhost"),
            OsString::from("--port"),
            OsString::from("443"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(
            tcp.payload,
            ToolPayload::HostPort {
                host: "localhost".into(),
                port: 443
            }
        );
        let mtr = ToolInvocation::parse([
            OsString::from("--tool"),
            OsString::from("mtr"),
            OsString::from("--host"),
            OsString::from("example.com"),
        ])
        .unwrap()
        .unwrap();
        assert_eq!(mtr.tool_id, "mtr");
        assert!(matches!(mtr.payload, ToolPayload::Host { .. }));
    }
}
