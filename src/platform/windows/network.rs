//! 通过 IP Helper API 枚举 TCP/UDP 与 IPv4/IPv6 端点。

use std::{
    mem::size_of,
    net::{Ipv4Addr, Ipv6Addr},
    slice,
};

use windows::Win32::{
    Foundation::ERROR_INSUFFICIENT_BUFFER,
    NetworkManagement::IpHelper::{
        GetExtendedTcpTable, GetExtendedUdpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID,
        MIB_UDP6ROW_OWNER_PID, MIB_UDPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL, UDP_TABLE_OWNER_PID,
    },
    Networking::WinSock::{AF_INET, AF_INET6},
};

use crate::{
    model::{AppError, IpVersion, NetworkEndpoint, NetworkProtocol, TcpState},
    platform::windows::process::snapshot_process_names,
};

/// 一次性读取所有 IP Helper 表，并用单次进程快照补充名称，避免端点级重复系统调用。
pub fn query_network_endpoints() -> Result<Vec<NetworkEndpoint>, AppError> {
    let names = snapshot_process_names()?;
    let mut endpoints = Vec::new();
    endpoints.extend(query_tcp_v4(&names)?);
    endpoints.extend(query_tcp_v6(&names)?);
    endpoints.extend(query_udp_v4(&names)?);
    endpoints.extend(query_udp_v6(&names)?);
    endpoints.sort_by(|left, right| {
        left.local_port
            .cmp(&right.local_port)
            .then_with(|| left.protocol.label().cmp(right.protocol.label()))
            .then_with(|| left.pid.cmp(&right.pid))
    });
    Ok(endpoints)
}

fn query_tcp_v4(
    names: &std::collections::HashMap<u32, String>,
) -> Result<Vec<NetworkEndpoint>, AppError> {
    let rows = read_tcp_rows::<MIB_TCPROW_OWNER_PID>(AF_INET.0 as u32)?;
    Ok(rows
        .iter()
        .map(|row| NetworkEndpoint {
            protocol: NetworkProtocol::Tcp,
            ip_version: IpVersion::V4,
            local_address: Ipv4Addr::from(u32::from_be(row.dwLocalAddr)).to_string(),
            local_port: decode_port(row.dwLocalPort),
            remote_address: Ipv4Addr::from(u32::from_be(row.dwRemoteAddr)).to_string(),
            remote_port: (row.dwRemotePort != 0).then(|| decode_port(row.dwRemotePort)),
            state: Some(tcp_state(row.dwState)),
            pid: row.dwOwningPid,
            process_name: names.get(&row.dwOwningPid).cloned().unwrap_or_default(),
        })
        .collect())
}

fn query_tcp_v6(
    names: &std::collections::HashMap<u32, String>,
) -> Result<Vec<NetworkEndpoint>, AppError> {
    let rows = read_tcp_rows::<MIB_TCP6ROW_OWNER_PID>(AF_INET6.0 as u32)?;
    Ok(rows
        .iter()
        .map(|row| NetworkEndpoint {
            protocol: NetworkProtocol::Tcp,
            ip_version: IpVersion::V6,
            local_address: Ipv6Addr::from(row.ucLocalAddr).to_string(),
            local_port: decode_port(row.dwLocalPort),
            remote_address: Ipv6Addr::from(row.ucRemoteAddr).to_string(),
            remote_port: (row.dwRemotePort != 0).then(|| decode_port(row.dwRemotePort)),
            state: Some(tcp_state(row.dwState)),
            pid: row.dwOwningPid,
            process_name: names.get(&row.dwOwningPid).cloned().unwrap_or_default(),
        })
        .collect())
}

fn query_udp_v4(
    names: &std::collections::HashMap<u32, String>,
) -> Result<Vec<NetworkEndpoint>, AppError> {
    let rows = read_udp_rows::<MIB_UDPROW_OWNER_PID>(AF_INET.0 as u32)?;
    Ok(rows
        .iter()
        .map(|row| NetworkEndpoint {
            protocol: NetworkProtocol::Udp,
            ip_version: IpVersion::V4,
            local_address: Ipv4Addr::from(u32::from_be(row.dwLocalAddr)).to_string(),
            local_port: decode_port(row.dwLocalPort),
            remote_address: String::new(),
            remote_port: None,
            state: None,
            pid: row.dwOwningPid,
            process_name: names.get(&row.dwOwningPid).cloned().unwrap_or_default(),
        })
        .collect())
}

fn query_udp_v6(
    names: &std::collections::HashMap<u32, String>,
) -> Result<Vec<NetworkEndpoint>, AppError> {
    let rows = read_udp_rows::<MIB_UDP6ROW_OWNER_PID>(AF_INET6.0 as u32)?;
    Ok(rows
        .iter()
        .map(|row| NetworkEndpoint {
            protocol: NetworkProtocol::Udp,
            ip_version: IpVersion::V6,
            local_address: Ipv6Addr::from(row.ucLocalAddr).to_string(),
            local_port: decode_port(row.dwLocalPort),
            remote_address: String::new(),
            remote_port: None,
            state: None,
            pid: row.dwOwningPid,
            process_name: names.get(&row.dwOwningPid).cloned().unwrap_or_default(),
        })
        .collect())
}

fn read_tcp_rows<T: Copy>(address_family: u32) -> Result<Vec<T>, AppError> {
    let mut size = 0_u32;
    // Safety: 首轮不提供指针；size 是有效可写地址，API 仅回填所需容量。
    let first = unsafe {
        GetExtendedTcpTable(
            None,
            &mut size,
            false,
            address_family,
            TCP_TABLE_OWNER_PID_ALL,
            0,
        )
    };
    if first != ERROR_INSUFFICIENT_BUFFER.0 && first != 0 {
        return Err(network_error("无法确定 TCP 表缓冲区大小", first));
    }
    read_rows(size, |buffer, actual_size| {
        // Safety: read_rows 提供的缓冲区按 u64 对齐、容量至少为 actual_size，回调期间保持有效。
        unsafe {
            GetExtendedTcpTable(
                Some(buffer),
                actual_size,
                false,
                address_family,
                TCP_TABLE_OWNER_PID_ALL,
                0,
            )
        }
    })
}

fn read_udp_rows<T: Copy>(address_family: u32) -> Result<Vec<T>, AppError> {
    let mut size = 0_u32;
    // Safety: 首轮不提供指针；size 是有效可写地址，API 仅回填所需容量。
    let first = unsafe {
        GetExtendedUdpTable(
            None,
            &mut size,
            false,
            address_family,
            UDP_TABLE_OWNER_PID,
            0,
        )
    };
    if first != ERROR_INSUFFICIENT_BUFFER.0 && first != 0 {
        return Err(network_error("无法确定 UDP 表缓冲区大小", first));
    }
    read_rows(size, |buffer, actual_size| {
        // Safety: read_rows 提供的缓冲区按 u64 对齐、容量至少为 actual_size，回调期间保持有效。
        unsafe {
            GetExtendedUdpTable(
                Some(buffer),
                actual_size,
                false,
                address_family,
                UDP_TABLE_OWNER_PID,
                0,
            )
        }
    })
}

fn read_rows<T: Copy>(
    size: u32,
    query: impl FnOnce(*mut core::ffi::c_void, &mut u32) -> u32,
) -> Result<Vec<T>, AppError> {
    if size < size_of::<u32>() as u32 {
        return Ok(Vec::new());
    }
    // 使用 u64 存储确保传给 C API 的起始地址至少 8 字节对齐，满足各 MIB 行结构的对齐要求。
    let words = (size as usize).div_ceil(size_of::<u64>());
    let mut buffer = vec![0_u64; words];
    let mut actual_size = size;
    let status = query(buffer.as_mut_ptr().cast(), &mut actual_size);
    if status != 0 {
        return Err(network_error("无法读取网络端点表", status));
    }

    // Safety: size 已验证至少包含表头，u64 缓冲区的起始地址满足 u32 对齐。
    let count = unsafe { *(buffer.as_ptr().cast::<u32>()) } as usize;
    let required = size_of::<u32>() + count.saturating_mul(size_of::<T>());
    if required > actual_size as usize {
        return Err(AppError::WindowsApi {
            context: "网络端点表返回了不完整的行数据".into(),
            code: format!("需要 {required} 字节，实际 {actual_size} 字节"),
        });
    }
    // Safety: required 已验证不超过 API 实际写入长度；起始地址按 T 的 ABI 对齐，且只读 count 行。
    let rows = unsafe {
        slice::from_raw_parts(
            buffer
                .as_ptr()
                .cast::<u8>()
                .add(size_of::<u32>())
                .cast::<T>(),
            count,
        )
    };
    // MIB 行仅含 Copy 数值和固定数组，复制后结果不再借用 API 缓冲区。
    Ok(rows.to_vec())
}

fn decode_port(network_order: u32) -> u16 {
    u16::from_be(network_order as u16)
}

fn tcp_state(value: u32) -> TcpState {
    match value {
        1 => TcpState::Closed,
        2 => TcpState::Listen,
        3 => TcpState::SynSent,
        4 => TcpState::SynReceived,
        5 => TcpState::Established,
        6 => TcpState::FinWait1,
        7 => TcpState::FinWait2,
        8 => TcpState::CloseWait,
        9 => TcpState::Closing,
        10 => TcpState::LastAck,
        11 => TcpState::TimeWait,
        12 => TcpState::DeleteTcb,
        _ => TcpState::Unknown,
    }
}

fn network_error(context: &str, code: u32) -> AppError {
    AppError::WindowsApi {
        context: context.into(),
        code: format!("Win32 error {code}"),
    }
}
