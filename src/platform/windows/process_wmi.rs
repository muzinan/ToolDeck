//! WMI 进程命令行查询。
//! 该模块只在后台线程按选中 PID 查询 `Win32_Process.CommandLine`，不负责快照、UI 或持久化。

use std::sync::OnceLock;

use windows::{
    Win32::System::{
        Variant::{VariantClear, VariantInit, VariantToString},
        Wmi::{
            IWbemClassObject, IWbemLocator, WBEM_E_ACCESS_DENIED, WBEM_E_NOT_FOUND,
            WBEM_GENERIC_FLAG_TYPE, WbemLocator,
        },
    },
    Win32::{
        Foundation::{E_ACCESSDENIED, RPC_E_TOO_LATE},
        System::{
            Com::{
                CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
                CoInitializeSecurity, CoSetProxyBlanket, CoUninitialize, EOAC_NONE,
                RPC_C_AUTHN_LEVEL_DEFAULT, RPC_C_AUTHN_LEVEL_PKT_PRIVACY,
                RPC_C_IMP_LEVEL_IMPERSONATE,
            },
            Rpc::{RPC_C_AUTHN_WINNT, RPC_C_AUTHZ_NONE},
        },
    },
    core::{BSTR, PCWSTR},
};

use crate::model::{AppError, ProcessCommandLine};

static WMI_SECURITY: OnceLock<Result<(), AppError>> = OnceLock::new();

struct ComApartment(bool);

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: 只有当前函数成功初始化的当前线程才会在 Drop 中配对释放 COM apartment。
            unsafe { CoUninitialize() };
        }
    }
}

/// 使用文档化 WMI 接口查询单个进程命令行；WMI 不可用时只失败当前详情请求。
pub fn query_process_command_line(pid: u32) -> Result<ProcessCommandLine, AppError> {
    let status = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    let _apartment = if status.is_ok() {
        ComApartment(true)
    } else {
        return Err(AppError::Unsupported(format!(
            "无法初始化 WMI 查询线程（HRESULT {}）",
            status.0
        )));
    };
    initialize_wmi_security()?;

    // SAFETY: WbemLocator 是系统注册的进程内 COM 类，返回接口由 windows crate 管理引用计数。
    let locator: IWbemLocator =
        unsafe { CoCreateInstance(&WbemLocator, None, CLSCTX_INPROC_SERVER) }
            .map_err(|error| wmi_error("无法创建 WMI 定位器", &error))?;
    let namespace = BSTR::from("ROOT\\CIMV2");
    // SAFETY: 所有 BSTR 在同步 COM 调用期间保持有效；None 表示使用默认 WMI 上下文。
    let services = unsafe {
        locator.ConnectServer(
            &namespace,
            &BSTR::new(),
            &BSTR::new(),
            &BSTR::new(),
            0,
            &BSTR::new(),
            None,
        )
    }
    .map_err(|error| wmi_error("无法连接 WMI ROOT\\CIMV2", &error))?;
    // SAFETY: services 是当前线程持有的 WMI 代理；默认凭据与模拟级别只作用于该代理调用。
    unsafe {
        CoSetProxyBlanket(
            &services,
            RPC_C_AUTHN_WINNT,
            RPC_C_AUTHZ_NONE,
            PCWSTR::null(),
            RPC_C_AUTHN_LEVEL_PKT_PRIVACY,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            None,
            EOAC_NONE,
        )
    }
    .map_err(|error| wmi_error("无法设置 WMI 代理安全级别", &error))?;

    let object_path = BSTR::from(format!("Win32_Process.Handle=\"{pid}\""));
    let mut object: Option<IWbemClassObject> = None;
    // SAFETY: 对象路径只含数字 PID；输出指针在同步调用期间有效，返回接口由 windows crate 管理引用计数。
    if let Err(error) = unsafe {
        services.GetObject(
            &object_path,
            WBEM_GENERIC_FLAG_TYPE(0),
            None,
            Some(&mut object),
            None,
        )
    } {
        if error.code().0 == WBEM_E_NOT_FOUND.0 {
            return Ok(ProcessCommandLine {
                pid,
                command_line: None,
            });
        }
        return Err(wmi_error("WMI 进程命令行查询失败", &error));
    }
    let Some(object) = object else {
        return Ok(ProcessCommandLine {
            pid,
            command_line: None,
        });
    };

    let mut value = unsafe { VariantInit() };
    let property_name = crate::platform::windows::wide::to_wide("CommandLine");
    // SAFETY: `value` 是 VariantInit 初始化的可写 VARIANT，object 在调用期间保持有效。
    let get_result =
        unsafe { object.Get(PCWSTR(property_name.as_ptr()), 0, &mut value, None, None) };
    if let Err(error) = get_result {
        unsafe { VariantClear(&mut value) }.ok();
        return Err(wmi_error("WMI 无法读取命令行属性", &error));
    }

    let mut buffer = vec![0_u16; 32_768];
    // SAFETY: buffer 是足够大的可写 UTF-16 缓冲区，VariantToString 只写入该缓冲区。
    let command_line = unsafe { VariantToString(&value, &mut buffer) }
        .ok()
        .map(|()| {
            let length = buffer
                .iter()
                .position(|value| *value == 0)
                .unwrap_or(buffer.len());
            String::from_utf16_lossy(&buffer[..length])
        })
        .filter(|value| !value.trim().is_empty());
    // SAFETY: value 由 VariantInit 初始化，且不再使用；必须在离开函数前清理其内部分配。
    unsafe { VariantClear(&mut value) }
        .map_err(|error| wmi_error("WMI 清理命令行结果失败", &error))?;

    Ok(ProcessCommandLine { pid, command_line })
}

fn initialize_wmi_security() -> Result<(), AppError> {
    WMI_SECURITY
        .get_or_init(|| {
            // SAFETY: COM 安全设置只初始化一次；None 使用当前进程令牌，模拟级别满足 WMI 本机查询要求。
            match unsafe {
                CoInitializeSecurity(
                    None,
                    -1,
                    None,
                    None,
                    RPC_C_AUTHN_LEVEL_DEFAULT,
                    RPC_C_IMP_LEVEL_IMPERSONATE,
                    None,
                    EOAC_NONE,
                    None,
                )
            } {
                Ok(()) => Ok(()),
                Err(error) if error.code() == RPC_E_TOO_LATE => Ok(()),
                Err(error) => Err(wmi_error("无法初始化 WMI 安全上下文", &error)),
            }
        })
        .clone()
}

fn wmi_error(context: &str, error: &windows::core::Error) -> AppError {
    if error.code() == E_ACCESSDENIED || error.code().0 == WBEM_E_ACCESS_DENIED.0 {
        AppError::AccessDenied(format!("{context}，WMI 拒绝访问进程数据"))
    } else {
        AppError::WindowsApi {
            context: context.into(),
            code: error.to_string(),
        }
    }
}
