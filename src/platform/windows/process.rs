//! 通过 Toolhelp 与受支持的进程 API 读取进程详情和父进程链。

use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::Instant,
};

use windows::{
    Win32::{
        Foundation::{CloseHandle, FILETIME, SYSTEMTIME},
        Security::{LookupAccountSidW, SID_NAME_USE, TOKEN_QUERY, TOKEN_USER, TokenUser},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
            SystemInformation::{GetSystemInfo, SYSTEM_INFO},
            Threading::{
                GetProcessTimes, OpenProcess, OpenProcessToken,
                PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, QueryFullProcessImageNameW,
                TerminateProcess,
            },
            Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime},
        },
    },
    core::PWSTR,
};

use crate::{
    model::{
        AppError, ProcessCommandLine, ProcessInfo, ProcessRunState, ProcessSummary,
        ProcessTreeNode, ProcessTreeSnapshot,
    },
    platform::windows::wide,
};

#[derive(Clone, Debug)]
struct SnapshotProcess {
    pid: u32,
    parent_pid: u32,
    name: String,
}

/// 上一次进程树快照的 CPU 计时基线；CPU 百分比必须由相邻两次采样求差得到。
/// 该缓存只在进程树刷新路径使用，键为 PID，值为累计核心时间与采样时刻。
static CPU_BASELINE: Mutex<Option<CpuBaseline>> = Mutex::new(None);

/// 一次进程树刷新时记录的 CPU 计时基线。
struct CpuBaseline {
    /// 采样时刻，用于计算两次快照之间的真实墙钟间隔。
    sampled_at: Instant,
    /// 每个 PID 的内核态与用户态累计时间之和，单位为 100 纳秒。
    totals: HashMap<u32, u64>,
}

/// 用 Drop 包装 HANDLE，使每个 OpenProcess 与快照句柄在所有错误路径释放。
struct WinHandle(windows::Win32::Foundation::HANDLE);

impl Drop for WinHandle {
    fn drop(&mut self) {
        // Safety: 句柄由 CreateToolhelp32Snapshot 或 OpenProcess 成功返回，所有权仅在此包装器中持有。
        let _ = unsafe { CloseHandle(self.0) };
    }
}

/// 获取完整进程详情，父链在一次快照内构建以避免多次查询产生不一致关系。
pub fn query_process(pid: u32) -> Result<ProcessInfo, AppError> {
    let processes = snapshot_processes()?;
    let by_pid: HashMap<u32, &SnapshotProcess> =
        processes.iter().map(|item| (item.pid, item)).collect();
    let current = by_pid.get(&pid).ok_or(AppError::ProcessExited(pid))?;
    let exe_path = query_exe_path(pid).ok();
    let started_at = query_started_at(pid).ok();
    let owner = query_owner(pid);

    let mut parent_chain = build_parent_chain(pid, &by_pid);
    fill_process_paths(&mut parent_chain);
    // 子进程列表需要状态列，逐个读取运行状态；所有权在此处不展示，保持 None 以免多余提权尝试。
    let mut children = processes
        .iter()
        .filter(|process| process.parent_pid == pid)
        .map(|process| ProcessSummary {
            pid: process.pid,
            name: process.name.clone(),
            exe_path: None,
            owner: None,
            started_at: query_started_at(process.pid).ok(),
            run_state: query_run_state(process.pid),
        })
        .collect::<Vec<_>>();
    fill_process_paths(&mut children);
    children.sort_by_key(|process| process.pid);

    Ok(ProcessInfo {
        pid,
        parent_pid: (current.parent_pid != 0).then_some(current.parent_pid),
        name: current.name.clone(),
        exe_path,
        command_line: None,
        started_at,
        owner,
        parent_chain,
        children,
    })
}

/// 获取全部存活进程的父子关系与资源占用；路径和命令行留给选中详情按需读取。
pub fn query_process_tree() -> Result<ProcessTreeSnapshot, AppError> {
    let processes = snapshot_processes()?;
    let mut snapshot = build_process_tree(processes);
    fill_tree_metrics(&mut snapshot);
    Ok(snapshot)
}

/// 为每个节点补齐 CPU、内存和运行状态。CPU 需要相邻两次采样求差，首次刷新只写入基线。
fn fill_tree_metrics(snapshot: &mut ProcessTreeSnapshot) {
    let sampled_at = Instant::now();
    let processors = f64::from(logical_processor_count());
    let mut totals = HashMap::with_capacity(snapshot.nodes.len());

    // 先整体取一次基线，避免逐个节点加锁导致同一次刷新内的时间基准不一致。
    let mut guard = CPU_BASELINE.lock().unwrap_or_else(|error| {
        // 互斥量被投毒说明上一次采样 panic，清除中毒状态后按首次采样继续。
        CPU_BASELINE.clear_poison();
        error.into_inner()
    });
    let previous = guard.take();
    let elapsed_ticks = previous.as_ref().map(|baseline| {
        // 100 纳秒为单位，与 GetProcessTimes 的计时粒度保持一致。
        sampled_at
            .duration_since(baseline.sampled_at)
            .as_nanos()
            .saturating_div(100) as u64
    });

    for node in &mut snapshot.nodes {
        node.memory_bytes = query_memory_bytes(node.pid);
        node.run_state = query_run_state(node.pid);
        let Some(total) = query_cpu_total(node.pid) else {
            continue;
        };
        totals.insert(node.pid, total);
        // 只有同一 PID 在上一轮存在基线且墙钟间隔为正时，CPU 百分比才有意义。
        if let Some(baseline) = previous.as_ref()
            && let Some(elapsed) = elapsed_ticks
            && elapsed > 0
            && let Some(before) = baseline.totals.get(&node.pid)
        {
            let used = total.saturating_sub(*before);
            let percent = (used as f64) * 100.0 / (elapsed as f64 * processors);
            node.cpu_percent = Some(percent.clamp(0.0, 100.0) as f32);
        }
    }
    *guard = Some(CpuBaseline { sampled_at, totals });
}

fn build_process_tree(processes: Vec<SnapshotProcess>) -> ProcessTreeSnapshot {
    let visible: HashSet<u32> = processes.iter().map(|process| process.pid).collect();
    let mut children_by_parent: HashMap<u32, Vec<u32>> = HashMap::new();

    for process in &processes {
        if visible.contains(&process.parent_pid) {
            children_by_parent
                .entry(process.parent_pid)
                .or_default()
                .push(process.pid);
        }
    }
    for children in children_by_parent.values_mut() {
        children.sort_unstable();
    }

    let mut nodes = processes
        .into_iter()
        .map(|process| ProcessTreeNode {
            pid: process.pid,
            parent_pid: (process.parent_pid != 0 && visible.contains(&process.parent_pid))
                .then_some(process.parent_pid),
            name: process.name,
            children: children_by_parent.remove(&process.pid).unwrap_or_default(),
            // 资源指标由 fill_tree_metrics 在树结构建立后统一补齐。
            cpu_percent: None,
            memory_bytes: None,
            run_state: ProcessRunState::default(),
        })
        .collect::<Vec<_>>();
    nodes.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then(left.pid.cmp(&right.pid))
    });
    let mut root_nodes = nodes
        .iter()
        .filter(|node| node.parent_pid.is_none())
        .collect::<Vec<_>>();
    root_nodes.sort_by(|left, right| {
        left.name
            .to_ascii_lowercase()
            .cmp(&right.name.to_ascii_lowercase())
            .then(left.pid.cmp(&right.pid))
    });
    let roots = root_nodes
        .into_iter()
        .map(|node| node.pid)
        .collect::<Vec<_>>();

    ProcessTreeSnapshot { nodes, roots }
}

/// 通过 WMI 懒加载指定进程命令行；具体查询保持在平台层，UI 只接收领域模型。
pub fn query_process_command_line(pid: u32) -> Result<ProcessCommandLine, AppError> {
    if !process_exists(pid)? {
        return Err(AppError::ProcessExited(pid));
    }
    let result = super::process_wmi::query_process_command_line(pid)?;
    // WMI 对已退出进程和未公开命令行的存活进程都可能返回空结果，因此用独立快照确认生命周期。
    if result.command_line.is_none() && !process_exists(pid)? {
        return Err(AppError::ProcessExited(pid));
    }
    Ok(result)
}

fn process_exists(pid: u32) -> Result<bool, AppError> {
    Ok(snapshot_processes()?
        .iter()
        .any(|process| process.pid == pid))
}

fn fill_process_paths(processes: &mut [ProcessSummary]) {
    for process in processes {
        process.exe_path = query_exe_path(process.pid).ok();
    }
}

/// 为其他平台服务提供低成本进程名称与路径查询。
pub(crate) fn query_process_summary(pid: u32) -> Result<ProcessSummary, AppError> {
    let processes = snapshot_processes()?;
    let process = processes
        .iter()
        .find(|item| item.pid == pid)
        .ok_or(AppError::ProcessExited(pid))?;
    Ok(ProcessSummary {
        pid,
        name: process.name.clone(),
        exe_path: query_exe_path(pid).ok(),
        owner: query_owner(pid),
        started_at: query_started_at(pid).ok(),
        run_state: query_run_state(pid),
    })
}

/// 获取当前进程快照，供端口表批量补充进程名称，避免每条端点重复 OpenProcess。
pub(crate) fn snapshot_process_names() -> Result<HashMap<u32, String>, AppError> {
    Ok(snapshot_processes()?
        .into_iter()
        .map(|process| (process.pid, process.name))
        .collect())
}

/// 结束指定用户进程；调用方必须已经完成 UI 二次确认。
pub fn terminate_process(pid: u32) -> Result<(), AppError> {
    if pid <= 4 {
        return Err(AppError::InvalidInput("系统保留 PID 不允许结束。".into()));
    }

    let info = query_process(pid)?;
    if is_protected_process(&info.name) {
        return Err(AppError::InvalidInput(format!(
            "系统关键进程 {} 不允许结束。",
            info.name
        )));
    }

    // Safety: PID 是调用方显式确认的正整数，OpenProcess 不接收悬垂指针，返回句柄由 WinHandle 接管。
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, false, pid) }
        .map(WinHandle)
        .map_err(|error| process_error(pid, "无法打开要结束的进程", &error))?;
    // Safety: handle.0 是 OpenProcess 成功返回且仍由局部 WinHandle 独占持有的有效进程句柄。
    unsafe { TerminateProcess(handle.0, 1) }
        .map_err(|error| process_error(pid, "无法结束进程", &error))
}

fn snapshot_processes() -> Result<Vec<SnapshotProcess>, AppError> {
    // Safety: 参数为受支持的快照标志与 PID 0；成功句柄立即交由 WinHandle 管理。
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }
        .map(WinHandle)
        .map_err(|error| api_error("无法创建进程快照", &error))?;
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut processes = Vec::new();

    // Safety: entry 的 dwSize 已按 Windows ABI 初始化，且快照句柄在循环期间保持有效。
    unsafe { Process32FirstW(snapshot.0, &mut entry) }
        .map_err(|error| api_error("无法读取进程快照", &error))?;
    loop {
        processes.push(SnapshotProcess {
            pid: entry.th32ProcessID,
            parent_pid: entry.th32ParentProcessID,
            name: wide::from_wide(&entry.szExeFile),
        });

        entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        // Safety: entry 再次按 ABI 初始化，快照句柄仍由 WinHandle 持有；API 只写入该结构体。
        match unsafe { Process32NextW(snapshot.0, &mut entry) } {
            Ok(()) => {}
            Err(error) if error.code().0 as u32 == 0x8007_0012 => break,
            Err(error) => return Err(api_error("读取进程快照时中断", &error)),
        }
    }
    Ok(processes)
}

/// 读取进程累计占用的核心时间，单位为 100 纳秒；无权限或进程已退出时返回 `None`。
fn query_cpu_total(pid: u32) -> Option<u64> {
    // Safety: PID 来自快照，OpenProcess 不接收原始指针，成功句柄由 WinHandle 管理。
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map(WinHandle)
        .ok()?;
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    // Safety: 四个 FILETIME 输出均为有效可写地址；句柄在查询完成前由 WinHandle 持有。
    unsafe { GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) }.ok()?;
    Some(filetime_to_ticks(kernel).saturating_add(filetime_to_ticks(user)))
}

/// 将 FILETIME 的高低 32 位合并为 100 纳秒计数。
const fn filetime_to_ticks(value: FILETIME) -> u64 {
    ((value.dwHighDateTime as u64) << 32) | value.dwLowDateTime as u64
}

/// 读取进程工作集大小，单位为字节；无权限或进程已退出时返回 `None`。
fn query_memory_bytes(pid: u32) -> Option<u64> {
    // Safety: PID 来自快照，OpenProcess 不接收原始指针，成功句柄由 WinHandle 管理。
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map(WinHandle)
        .ok()?;
    let mut counters = PROCESS_MEMORY_COUNTERS::default();
    let size = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    // Safety: counters 为有效可写结构体，size 与其真实大小一致；句柄由 WinHandle 保持有效。
    unsafe { GetProcessMemoryInfo(handle.0, &mut counters, size) }.ok()?;
    Some(counters.WorkingSetSize as u64)
}

/// 读取进程令牌所属账户，返回 `域\用户`；无权限或查询失败时返回 `None`。
fn query_owner(pid: u32) -> Option<String> {
    // Safety: PID 来自快照，OpenProcess 不接收原始指针，成功句柄由 WinHandle 管理。
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map(WinHandle)
        .ok()?;
    let mut token = windows::Win32::Foundation::HANDLE::default();
    // Safety: token 为有效可写句柄地址；成功返回的令牌立即交由 WinHandle 释放。
    unsafe { OpenProcessToken(process.0, TOKEN_QUERY, &mut token) }.ok()?;
    let token = WinHandle(token);

    // 先用零长度探测所需缓冲区大小，再按返回长度分配，避免固定长度截断 SID。
    let mut needed = 0_u32;
    // Safety: 允许失败；该调用只写入 needed，不解引用空缓冲区。
    let _ = unsafe {
        windows::Win32::Security::GetTokenInformation(token.0, TokenUser, None, 0, &mut needed)
    };
    if needed == 0 {
        return None;
    }
    let mut buffer = vec![0_u8; needed as usize];
    // Safety: buffer 至少有 needed 字节可写，长度参数与其容量一致。
    unsafe {
        windows::Win32::Security::GetTokenInformation(
            token.0,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
    }
    .ok()?;

    // Safety: buffer 由 GetTokenInformation 按 TOKEN_USER 布局填充，且容量已按其返回长度分配。
    let token_user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let sid = token_user.User.Sid;
    if sid.is_invalid() {
        return None;
    }

    // 同样先探测名称与域名长度，Windows 在缓冲区不足时通过输出参数回报所需长度。
    let mut name_len = 0_u32;
    let mut domain_len = 0_u32;
    let mut sid_use = SID_NAME_USE::default();
    // Safety: 允许失败；两个长度参数为有效可写地址，名称缓冲区传 None 表示仅探测长度。
    let _ = unsafe {
        LookupAccountSidW(
            None,
            sid,
            None,
            &mut name_len,
            None,
            &mut domain_len,
            &mut sid_use,
        )
    };
    if name_len == 0 {
        return None;
    }
    let mut name = vec![0_u16; name_len as usize];
    let mut domain = vec![0_u16; domain_len.max(1) as usize];
    // Safety: 两个缓冲区分别有 name_len 与 domain_len 个可写 UTF-16 单元，长度参数与容量一致。
    unsafe {
        LookupAccountSidW(
            None,
            sid,
            Some(PWSTR(name.as_mut_ptr())),
            &mut name_len,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_len,
            &mut sid_use,
        )
    }
    .ok()?;

    let account = wide::from_wide(&name);
    let domain = wide::from_wide(&domain);
    if account.is_empty() {
        None
    } else if domain.is_empty() {
        Some(account)
    } else {
        Some(format!("{domain}\\{account}"))
    }
}

/// 判定进程在快照时刻的可见运行状态；无法打开进程时按未知处理。
fn query_run_state(pid: u32) -> ProcessRunState {
    // Safety: PID 来自快照，OpenProcess 不接收原始指针，成功句柄由 WinHandle 管理。
    let opened = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) };
    let Ok(handle) = opened else {
        return ProcessRunState::Unknown;
    };
    let handle = WinHandle(handle);
    let mut code = 0_u32;
    // Safety: code 为有效可写地址；句柄在查询完成前由 WinHandle 持有。
    if unsafe { windows::Win32::System::Threading::GetExitCodeProcess(handle.0, &mut code) }
        .is_err()
    {
        return ProcessRunState::Unknown;
    }
    // STILL_ACTIVE(259) 表示进程尚未退出；其余退出码说明进程在读取瞬间已结束。
    if code == 259 {
        ProcessRunState::Running
    } else {
        ProcessRunState::Unknown
    }
}

/// 读取逻辑处理器数量，用于把核心时间归一为单机 CPU 百分比。
fn logical_processor_count() -> u32 {
    let mut info = SYSTEM_INFO::default();
    // Safety: info 为有效可写结构体，GetSystemInfo 只写入该结构体。
    unsafe { GetSystemInfo(&mut info) };
    info.dwNumberOfProcessors.max(1)
}

fn query_exe_path(pid: u32) -> Result<String, AppError> {
    // Safety: PID 来自快照，OpenProcess 不接收原始指针，成功句柄由 WinHandle 管理。
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map(WinHandle)
        .map_err(|error| process_error(pid, "无法打开进程", &error))?;
    let mut buffer = vec![0_u16; 32_768];
    let mut length = buffer.len() as u32;

    // Safety: buffer 指向 32,768 个可写 UTF-16 单元，length 与其容量一致，句柄由 WinHandle 保持有效。
    unsafe {
        QueryFullProcessImageNameW(
            handle.0,
            Default::default(),
            PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    }
    .map_err(|error| process_error(pid, "无法读取进程路径", &error))?;
    Ok(String::from_utf16_lossy(&buffer[..length as usize]))
}

fn query_started_at(pid: u32) -> Result<String, AppError> {
    // Safety: PID 来自快照，OpenProcess 不接收原始指针，成功句柄由 WinHandle 管理。
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .map(WinHandle)
        .map_err(|error| process_error(pid, "无法读取进程启动时间", &error))?;
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();

    // Safety: 四个 FILETIME 输出均为有效可写地址；句柄在查询完成前由 WinHandle 持有。
    unsafe { GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) }
        .map_err(|error| process_error(pid, "无法读取进程启动时间", &error))?;
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    // Safety: created 是 GetProcessTimes 写入的有效 FILETIME，utc 是有效可写 SYSTEMTIME。
    unsafe { FileTimeToSystemTime(&created, &mut utc) }
        .map_err(|error| api_error("无法转换进程启动时间", &error))?;
    // Safety: utc 和 local 都是有效的 SYSTEMTIME 地址；None 表示使用系统当前时区。
    unsafe { SystemTimeToTzSpecificLocalTime(None, &utc, &mut local) }
        .map_err(|error| api_error("无法转换本地时间", &error))?;
    Ok(format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond
    ))
}

fn build_parent_chain(pid: u32, by_pid: &HashMap<u32, &SnapshotProcess>) -> Vec<ProcessSummary> {
    let mut chain = Vec::new();
    let mut visited = HashSet::new();
    let mut cursor = pid;

    while let Some(process) = by_pid.get(&cursor) {
        if !visited.insert(cursor) {
            break;
        }
        // 父链只展示名称与路径，不展示所有权和状态，避免为每一级父进程重复提权查询。
        chain.push(ProcessSummary {
            pid: process.pid,
            name: process.name.clone(),
            exe_path: None,
            owner: None,
            started_at: None,
            run_state: ProcessRunState::default(),
        });
        if process.parent_pid == 0 {
            break;
        }
        cursor = process.parent_pid;
    }
    chain.reverse();
    chain
}

fn is_protected_process(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "system"
            | "registry"
            | "smss.exe"
            | "csrss.exe"
            | "wininit.exe"
            | "winlogon.exe"
            | "services.exe"
            | "lsass.exe"
    )
}

fn api_error(context: &str, error: &windows::core::Error) -> AppError {
    AppError::WindowsApi {
        context: context.into(),
        code: error.to_string(),
    }
}

fn process_error(pid: u32, context: &str, error: &windows::core::Error) -> AppError {
    let detail = error.to_string();
    if error.code().0 as u32 == 0x8007_0005 {
        AppError::AccessDenied(format!("{context}，PID {pid}"))
    } else {
        AppError::WindowsApi {
            context: format!("{context}，PID {pid}"),
            code: detail,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        SnapshotProcess, build_parent_chain, build_process_tree, query_process_command_line,
    };

    #[test]
    fn builds_chain_from_root_to_target() {
        let values = [
            SnapshotProcess {
                pid: 1,
                parent_pid: 0,
                name: "root.exe".into(),
            },
            SnapshotProcess {
                pid: 2,
                parent_pid: 1,
                name: "parent.exe".into(),
            },
            SnapshotProcess {
                pid: 3,
                parent_pid: 2,
                name: "child.exe".into(),
            },
        ];
        let by_pid: HashMap<u32, &SnapshotProcess> =
            values.iter().map(|item| (item.pid, item)).collect();
        let chain = build_parent_chain(3, &by_pid);

        assert_eq!(
            chain.iter().map(|item| item.pid).collect::<Vec<_>>(),
            [1, 2, 3]
        );
    }

    #[test]
    fn breaks_cycles_in_snapshot() {
        let values = [
            SnapshotProcess {
                pid: 1,
                parent_pid: 2,
                name: "a.exe".into(),
            },
            SnapshotProcess {
                pid: 2,
                parent_pid: 1,
                name: "b.exe".into(),
            },
        ];
        let by_pid: HashMap<u32, &SnapshotProcess> =
            values.iter().map(|item| (item.pid, item)).collect();
        assert_eq!(build_parent_chain(1, &by_pid).len(), 2);
    }

    #[test]
    fn builds_orphan_roots_and_stable_name_sorting() {
        let snapshot = build_process_tree(vec![
            SnapshotProcess {
                pid: 30,
                parent_pid: 999,
                name: "zeta.exe".into(),
            },
            SnapshotProcess {
                pid: 40,
                parent_pid: 0,
                name: "alpha.exe".into(),
            },
            SnapshotProcess {
                pid: 21,
                parent_pid: 40,
                name: "child.exe".into(),
            },
        ]);
        assert_eq!(snapshot.roots, [40, 30]);
        assert_eq!(snapshot.nodes[0].name, "alpha.exe");
        assert_eq!(snapshot.nodes[0].children, [21]);
        assert_eq!(snapshot.nodes[2].parent_pid, None);
    }

    #[test]
    fn live_process_is_never_misreported_as_exited() {
        let pid = std::process::id();
        let result = query_process_command_line(pid);

        assert!(!matches!(
            result,
            Err(crate::model::AppError::ProcessExited(_))
        ));
        if let Ok(result) = result {
            assert_eq!(result.pid, pid);
            assert!(
                result
                    .command_line
                    .as_deref()
                    .is_some_and(|value| !value.trim().is_empty())
            );
        }
    }

    #[test]
    fn reports_missing_process_with_requested_pid() {
        let error = query_process_command_line(u32::MAX).expect_err("无效 PID 不应返回命令行");

        assert!(matches!(
            error,
            crate::model::AppError::ProcessExited(u32::MAX)
        ));
    }
}
