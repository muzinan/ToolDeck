//! DNS、ICMP 和 TCP 探测的平台适配。
//! Win32 指针和句柄只在这里处理，向上层返回可序列化的领域模型。
#![allow(unsafe_op_in_unsafe_fn)]

use std::{
    net::{IpAddr, Ipv6Addr, SocketAddr, TcpStream, ToSocketAddrs},
    time::{Duration, Instant},
};

use windows::{
    Win32::{
        Foundation::{DNS_ERROR_RCODE_NAME_ERROR, DNS_ERROR_RCODE_NXRRSET, ERROR_TIMEOUT},
        NetworkManagement::{
            Dns::{
                DNS_QUERY_BYPASS_CACHE, DNS_QUERY_OPTIONS, DNS_RECORDA, DNS_TYPE, DNS_TYPE_A,
                DNS_TYPE_AAAA, DNS_TYPE_CNAME, DNS_TYPE_MX, DNS_TYPE_NS, DNS_TYPE_PTR,
                DNS_TYPE_TEXT, DnsFree, DnsFreeRecordList, DnsQuery_A,
            },
            IpHelper::{
                ICMP_ECHO_REPLY, ICMPV6_ECHO_REPLY_LH, IP_OPTION_INFORMATION, Icmp6CreateFile,
                Icmp6SendEcho2, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho,
            },
        },
        Networking::WinSock::{ADDRESS_FAMILY, AF_INET6, IN6_ADDR, IN6_ADDR_0, SOCKADDR_IN6},
    },
    core::{PCSTR, PSTR},
};

use crate::model::{
    AppError, DnsRecord, DnsRecordType, DnsResult, PingAddressFamily, PingSample, PingSummary,
    TcpProbeAttempt, TcpProbeResult,
};

/// 使用 Windows 系统 DNS 查询记录，支持系统缓存和绕过缓存两种模式。
pub fn query_dns(
    host: &str,
    record_type: DnsRecordType,
    bypass_cache: bool,
) -> Result<DnsResult, AppError> {
    let host = host.trim();
    if host.is_empty() {
        return Err(AppError::InvalidInput("主机名不能为空。".into()));
    }
    let selected = if record_type == DnsRecordType::Auto {
        if host.parse::<IpAddr>().is_ok() {
            DnsRecordType::Ptr
        } else {
            DnsRecordType::A
        }
    } else {
        record_type
    };
    let mut records = query_dns_type(host, selected, bypass_cache)?;
    if record_type == DnsRecordType::Auto && host.parse::<IpAddr>().is_err() {
        records.extend(query_dns_type(host, DnsRecordType::Aaaa, bypass_cache)?);
    }
    Ok(DnsResult {
        host: host.to_owned(),
        records,
    })
}

fn query_dns_type(
    host: &str,
    record_type: DnsRecordType,
    bypass_cache: bool,
) -> Result<Vec<DnsRecord>, AppError> {
    let name = std::ffi::CString::new(host)
        .map_err(|_| AppError::InvalidInput("主机名包含无效字符。".into()))?;
    let mut list: *mut DNS_RECORDA = std::ptr::null_mut();
    let options = if bypass_cache {
        DNS_QUERY_BYPASS_CACHE
    } else {
        DNS_QUERY_OPTIONS(0)
    };
    let status = unsafe {
        DnsQuery_A(
            PCSTR(name.as_ptr().cast()),
            dns_type(record_type),
            options,
            None,
            &mut list,
            None,
        )
    };
    if status.0 != 0 {
        let error = match status.0 {
            code if code == DNS_ERROR_RCODE_NAME_ERROR.0 => {
                AppError::NameResolution(format!("NXDOMAIN：找不到主机 {}。", host))
            }
            code if code == DNS_ERROR_RCODE_NXRRSET.0 => {
                AppError::NameResolution(format!("主机存在，但没有 {} 记录。", record_type.label()))
            }
            code if code == ERROR_TIMEOUT.0 => AppError::Timeout("系统 DNS 查询超时。".into()),
            code => AppError::NameResolution(format!(
                "查询 {} 记录失败，Windows DNS 错误码 {}。",
                record_type.label(),
                code
            )),
        };
        return Err(error);
    }
    let mut records = Vec::new();
    let mut current = list;
    while !current.is_null() {
        let record = unsafe { &*current };
        if let Some(value) = unsafe { record_value(record, record_type) } {
            records.push(DnsRecord {
                name: unsafe { pwstr(record.pName) },
                record_type,
                value,
                ttl: record.dwTtl,
            });
        }
        current = record.pNext;
    }
    if !list.is_null() {
        unsafe { DnsFree(Some(list.cast()), DnsFreeRecordList) };
    }
    Ok(records)
}

fn dns_type(value: DnsRecordType) -> DNS_TYPE {
    match value {
        DnsRecordType::Auto | DnsRecordType::A => DNS_TYPE_A,
        DnsRecordType::Aaaa => DNS_TYPE_AAAA,
        DnsRecordType::Cname => DNS_TYPE_CNAME,
        DnsRecordType::Mx => DNS_TYPE_MX,
        DnsRecordType::Txt => DNS_TYPE_TEXT,
        DnsRecordType::Ns => DNS_TYPE_NS,
        DnsRecordType::Ptr => DNS_TYPE_PTR,
    }
}

unsafe fn record_value(record: &DNS_RECORDA, record_type: DnsRecordType) -> Option<String> {
    match record_type {
        DnsRecordType::A => {
            Some(std::net::Ipv4Addr::from(record.Data.A.IpAddress.to_be_bytes()).to_string())
        }
        DnsRecordType::Aaaa => {
            Some(Ipv6Addr::from(record.Data.AAAA.Ip6Address.IP6Byte).to_string())
        }
        DnsRecordType::Cname | DnsRecordType::Ns | DnsRecordType::Ptr => {
            Some(pwstr(record.Data.PTR.pNameHost))
        }
        DnsRecordType::Mx => Some(format!(
            "{} (优先级 {})",
            pwstr(record.Data.MX.pNameExchange),
            record.Data.MX.wPreference
        )),
        DnsRecordType::Txt => {
            let data = record.Data.TXT;
            let values =
                std::slice::from_raw_parts(data.pStringArray.as_ptr(), data.dwStringCount as usize)
                    .iter()
                    .map(|value| pwstr(*value))
                    .collect::<Vec<_>>();
            Some(values.join(" "))
        }
        DnsRecordType::Auto => None,
    }
}

unsafe fn pwstr(value: PSTR) -> String {
    value
        .to_string()
        .unwrap_or_default()
        .trim_end_matches('\0')
        .to_owned()
}

/// 使用 IcmpSendEcho/Icmp6SendEcho2 执行指定次数的 ICMP 回显。
pub fn ping_host(
    host: &str,
    count: u32,
    timeout_ms: u32,
    payload_size: u16,
    family: PingAddressFamily,
) -> Result<PingSummary, AppError> {
    ping_host_with_progress(
        host,
        count,
        timeout_ms,
        payload_size,
        family,
        |_sample, _completed, _total| {},
    )
}

/// 执行 ICMP 回显并在每个样本完成后回调进度，回调只运行在当前 Worker 线程。
pub fn ping_host_with_progress<F>(
    host: &str,
    count: u32,
    timeout_ms: u32,
    payload_size: u16,
    family: PingAddressFamily,
    mut on_sample: F,
) -> Result<PingSummary, AppError>
where
    F: FnMut(&PingSample, u32, u32),
{
    let address = (host, 0)
        .to_socket_addrs()
        .map_err(|error| AppError::NameResolution(format!("{host}：{error}")))?
        .find(|value| match family {
            PingAddressFamily::Auto => true,
            PingAddressFamily::V4 => value.is_ipv4(),
            PingAddressFamily::V6 => value.is_ipv6(),
        })
        .ok_or_else(|| AppError::NameResolution("没有可用的地址族。".into()))?;
    let count = count.clamp(1, 10);
    let mut samples = Vec::with_capacity(count as usize);
    for index in 0..count {
        let started = Instant::now();
        let result = ping_once(
            address,
            timeout_ms.clamp(100, 10_000),
            payload_size.min(1472),
        );
        let elapsed = started.elapsed().as_secs_f64() * 1000.0;
        match result {
            Ok(ttl) => samples.push(PingSample {
                address: address.ip().to_string(),
                elapsed_ms: Some(elapsed),
                ttl,
                error: None,
            }),
            Err(error) => samples.push(PingSample {
                address: address.ip().to_string(),
                elapsed_ms: None,
                ttl: None,
                error: Some(error),
            }),
        }
        let sample = samples.last().expect("刚追加的 Ping 样本必须存在");
        on_sample(sample, index + 1, count);
    }
    let values = samples
        .iter()
        .filter_map(|sample| sample.elapsed_ms)
        .collect::<Vec<_>>();
    let sent = samples.len() as u32;
    let received = values.len() as u32;
    let avg_ms = (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64);
    Ok(PingSummary {
        host: host.to_owned(),
        samples,
        sent,
        received,
        min_ms: values.iter().copied().reduce(f64::min),
        avg_ms,
        max_ms: values.iter().copied().reduce(f64::max),
    })
}

fn ping_once(
    address: SocketAddr,
    timeout_ms: u32,
    payload_size: u16,
) -> Result<Option<u8>, String> {
    let payload = vec![0_u8; payload_size as usize];
    let mut reply = vec![0_u8; std::mem::size_of::<ICMP_ECHO_REPLY>() + payload.len() + 64];
    match address.ip() {
        IpAddr::V4(value) => {
            let handle = unsafe { IcmpCreateFile() }.map_err(|error| error.to_string())?;
            let status = unsafe {
                IcmpSendEcho(
                    handle,
                    u32::from(value),
                    payload.as_ptr().cast(),
                    payload_size,
                    None,
                    reply.as_mut_ptr().cast(),
                    reply.len() as u32,
                    timeout_ms,
                )
            };
            unsafe { IcmpCloseHandle(handle) }.ok();
            if status == 0 {
                return Err("请求超时或主机不可达".into());
            }
            let response = unsafe { &*(reply.as_ptr().cast::<ICMP_ECHO_REPLY>()) };
            if response.Status != 0 {
                return Err(format!("ICMP 响应状态 {}", response.Status));
            }
            Ok(Some(response.Options.Ttl))
        }
        IpAddr::V6(value) => {
            let handle = unsafe { Icmp6CreateFile() }.map_err(|error| error.to_string())?;
            let destination = SOCKADDR_IN6 {
                sin6_family: ADDRESS_FAMILY(AF_INET6.0),
                sin6_addr: IN6_ADDR {
                    u: IN6_ADDR_0 {
                        Byte: value.octets(),
                    },
                },
                ..Default::default()
            };
            let options = IP_OPTION_INFORMATION::default();
            let status = unsafe {
                Icmp6SendEcho2(
                    handle,
                    None,
                    None,
                    None,
                    std::ptr::null(),
                    &destination,
                    payload.as_ptr().cast(),
                    payload_size,
                    Some(&options),
                    reply.as_mut_ptr().cast(),
                    reply.len() as u32,
                    timeout_ms,
                )
            };
            unsafe { IcmpCloseHandle(handle) }.ok();
            if status == 0 {
                return Err("请求超时或主机不可达".into());
            }
            let response = unsafe { &*(reply.as_ptr().cast::<ICMPV6_ECHO_REPLY_LH>()) };
            if response.Status != 0 {
                return Err(format!("ICMPv6 响应状态 {}", response.Status));
            }
            Ok(None)
        }
    }
}

/// 在总截止时间内依次尝试解析出的每个 TCP 地址。
pub fn tcp_probe(host: &str, port: u16, timeout_ms: u32) -> Result<TcpProbeResult, AppError> {
    if port == 0 {
        return Err(AppError::InvalidInput("端口必须大于 0。".into()));
    }
    let mut addresses = (host, port)
        .to_socket_addrs()
        .map_err(|error| AppError::NameResolution(format!("{host}：{error}")))?
        .collect::<Vec<_>>();
    addresses.sort_by_key(|address| address.is_ipv6());
    addresses.dedup();
    let deadline = Instant::now() + Duration::from_millis(timeout_ms.clamp(100, 30_000) as u64);
    let mut attempts = Vec::new();
    let mut success = false;
    for address in addresses {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let started = Instant::now();
        let status = match TcpStream::connect_timeout(&address, remaining) {
            Ok(stream) => {
                drop(stream);
                success = true;
                "成功".to_owned()
            }
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => "拒绝".into(),
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => "超时".into(),
            Err(_) => "不可达".into(),
        };
        attempts.push(TcpProbeAttempt {
            address: address.to_string(),
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            status,
        });
        if success {
            break;
        }
    }
    Ok(TcpProbeResult {
        host: host.to_owned(),
        port,
        attempts,
        success,
    })
}

#[cfg(test)]
mod tests {
    use std::{net::TcpListener, thread};

    use super::tcp_probe;

    #[test]
    fn tcp_probe_connects_to_a_local_listener() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let acceptor = thread::spawn(move || listener.accept().is_ok());

        let result = tcp_probe("127.0.0.1", port, 1_000).unwrap();

        assert!(result.success);
        assert_eq!(result.attempts.len(), 1);
        assert!(acceptor.join().unwrap());
    }

    #[test]
    fn tcp_probe_rejects_zero_port_before_resolution() {
        assert!(tcp_probe("127.0.0.1", 0, 1_000).is_err());
    }
}
