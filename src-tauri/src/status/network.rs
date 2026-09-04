//! 网络指标与代理检测，对标 `cmd/status/metrics_network.go`。

use super::types::ProxyStatus;
use std::collections::HashMap;
use std::time::Duration;

/// 对标 `noiseInterfacePrefixes`。
const NOISE_INTERFACE_PREFIXES: &[&str] = &[
    "lo", "awdl", "utun", "llw", "bridge", "gif", "stf", "xhc", "anpi", "ap",
];

const AF_LINK: u32 = 18;
const AF_INET: u32 = 2;

/// 对标 `collectIOCountersSafely`：读取每接口字节计数。
///
/// 原实现经 gopsutil（sysctl NET_RT_IFLIST2）；这里走 getifaddrs 的
/// AF_LINK `if_data64`（ifi_ibytes/ifi_obytes，同为内核 64 位计数器）。
pub fn interface_counters() -> Option<HashMap<String, (u64, u64)>> {
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return None;
        }
        let mut result: HashMap<String, (u64, u64)> = HashMap::new();
        let mut cur = ifap;
        while !cur.is_null() {
            let ifa = &*cur;
            let family = if !ifa.ifa_addr.is_null() {
                (*ifa.ifa_addr).sa_family as u32
            } else {
                0
            };
            if family == AF_LINK && !ifa.ifa_data.is_null() {
                let name = cstr(ifa.ifa_name);
                // if_data64：ifi_ibytes 偏移 24，ifi_obytes 偏移 32
                // （net/if_var.h 结构序）。libc 的 if_data64 与该布局一致。
                let data = &*(ifa.ifa_data as *const libc::if_data64);
                result.entry(name).or_insert((data.ifi_ibytes, data.ifi_obytes));
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
        Some(result)
    }
}

/// 对标 `getInterfaceIPs`：每接口第一个非环回 IPv4。
pub fn interface_ips() -> HashMap<String, String> {
    let mut result = HashMap::new();
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return result;
        }
        let mut cur = ifap;
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.ifa_addr.is_null() && (*ifa.ifa_addr).sa_family as u32 == AF_INET {
                let name = cstr(ifa.ifa_name);
                let sin = ifa.ifa_addr as *const libc::sockaddr_in;
                let octets = (*sin).sin_addr.s_addr.to_be_bytes();
                let ip = format!("{}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3]);
                // IPv4 only，排除 127.*。
                if !ip.starts_with("127.") && !result.contains_key(&name) {
                    result.insert(name, ip);
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
    }
    result
}

/// 对标 `isNoiseInterface`。
pub fn is_noise_interface(name: &str) -> bool {
    let lower = name.to_lowercase();
    NOISE_INTERFACE_PREFIXES
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

/// 对标 `counterDelta`。
pub fn counter_delta(current: u64, previous: u64) -> u64 {
    current.saturating_sub(previous)
}

unsafe fn cstr(ptr: *const libc::c_char) -> String {
    std::ffi::CStr::from_ptr(ptr).to_string_lossy().into_owned()
}

/// 对标 `collectProxy`：环境变量 → scutil 系统代理 → utun 隧道提示。
pub fn collect_proxy() -> ProxyStatus {
    let proxy = collect_proxy_from_env();
    if proxy.enabled {
        return proxy;
    }

    if let Ok(out) = super::run_cmd("scutil", &["--proxy"], Duration::from_millis(500)) {
        let proxy = collect_proxy_from_scutil_output(&out);
        if proxy.enabled {
            return proxy;
        }
    }

    if let Some(proxy) = collect_proxy_from_tun_interfaces() {
        return proxy;
    }

    ProxyStatus::default()
}

/// 1:1 移植 `collectProxyFromEnv`（含 ALL_PROXY）。
pub fn collect_proxy_from_env() -> ProxyStatus {
    let env_keys = [
        "https_proxy",
        "HTTPS_PROXY",
        "http_proxy",
        "HTTP_PROXY",
        "all_proxy",
        "ALL_PROXY",
    ];
    for key in env_keys {
        let Ok(val) = std::env::var(key) else {
            continue;
        };
        let val = val.trim();
        if val.is_empty() {
            continue;
        }
        let proxy_type = if val.to_lowercase().starts_with("socks") {
            "SOCKS"
        } else {
            "HTTP"
        };
        let host = {
            let parsed = parse_proxy_host(val);
            if parsed.is_empty() {
                val.to_string()
            } else {
                parsed
            }
        };
        return ProxyStatus {
            enabled: true,
            kind: proxy_type.into(),
            host,
            is_tunnel: false,
        };
    }
    ProxyStatus::default()
}

/// 1:1 移植 `collectProxyFromScutilOutput`（检查顺序敏感）。
pub fn collect_proxy_from_scutil_output(out: &str) -> ProxyStatus {
    if out.is_empty() {
        return ProxyStatus::default();
    }

    if scutil_proxy_value(out, "SOCKSEnable") == "1" {
        let host = join_host_port(
            &scutil_proxy_value(out, "SOCKSProxy"),
            &scutil_proxy_value(out, "SOCKSPort"),
        );
        return ProxyStatus {
            enabled: true,
            kind: "SOCKS".into(),
            host: if host.is_empty() { "System Proxy".into() } else { host },
            is_tunnel: false,
        };
    }
    if scutil_proxy_value(out, "HTTPSEnable") == "1" {
        let host = join_host_port(
            &scutil_proxy_value(out, "HTTPSProxy"),
            &scutil_proxy_value(out, "HTTPSPort"),
        );
        return ProxyStatus {
            enabled: true,
            kind: "HTTPS".into(),
            host: if host.is_empty() { "System Proxy".into() } else { host },
            is_tunnel: false,
        };
    }
    if scutil_proxy_value(out, "HTTPEnable") == "1" {
        let host = join_host_port(
            &scutil_proxy_value(out, "HTTPProxy"),
            &scutil_proxy_value(out, "HTTPPort"),
        );
        return ProxyStatus {
            enabled: true,
            kind: "HTTP".into(),
            host: if host.is_empty() { "System Proxy".into() } else { host },
            is_tunnel: false,
        };
    }
    if scutil_proxy_value(out, "ProxyAutoConfigEnable") == "1" {
        let pac_url = scutil_proxy_value(out, "ProxyAutoConfigURLString");
        let host = parse_proxy_host(&pac_url);
        return ProxyStatus {
            enabled: true,
            kind: "PAC".into(),
            host: if host.is_empty() { "PAC".into() } else { host },
            is_tunnel: false,
        };
    }
    if scutil_proxy_value(out, "ProxyAutoDiscoveryEnable") == "1" {
        return ProxyStatus {
            enabled: true,
            kind: "WPAD".into(),
            host: "Auto Discovery".into(),
            is_tunnel: false,
        };
    }
    ProxyStatus::default()
}

/// 1:1 移植 `collectProxyFromTunInterfaces`：仅存在活动 utun/tun 时不标注
/// 为代理（可能只是 iCloud 私密转送/VPN），以 IsTunnel 提示呈现。
pub fn collect_proxy_from_tun_interfaces() -> Option<ProxyStatus> {
    let counters = interface_counters()?;
    let mut active_tun: Vec<String> = counters
        .iter()
        .filter(|(name, (rx, tx))| {
            let lower = name.to_lowercase();
            (lower.starts_with("utun") || lower.starts_with("tun")) && rx + tx > 0
        })
        .map(|(name, _)| name.clone())
        .collect();
    if active_tun.is_empty() {
        return None;
    }
    active_tun.sort();
    let host = if active_tun.len() > 1 {
        format!("{}+", active_tun[0])
    } else {
        active_tun[0].clone()
    };
    Some(ProxyStatus {
        enabled: true,
        kind: "TUN".into(),
        host,
        is_tunnel: true,
    })
}

/// 1:1 移植 `scutilProxyValue`。
pub fn scutil_proxy_value(out: &str, key: &str) -> String {
    let prefix = format!("{key} :");
    for line in out.lines() {
        let line = line.trim();
        if let Some(after) = line.strip_prefix(&prefix) {
            return after.trim().to_string();
        }
    }
    String::new()
}

/// 1:1 移植 `parseProxyHost`：等价 url.Parse 取 Host 语义
/// （去 scheme、去路径/查询、去 userinfo）。
pub fn parse_proxy_host(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() {
        return String::new();
    }
    let target = if !raw.contains("://") {
        format!("http://{raw}")
    } else {
        raw.to_string()
    };
    // 去 scheme。
    let rest = target.split_once("://").map(|(_, r)| r).unwrap_or(&target);
    // 截断路径/查询/片段。
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    // Go url.Parse 的 Host 不含 userinfo。
    let host = authority.rsplit('@').next().unwrap_or_default();
    host.trim_start_matches('@').to_string()
}

/// 1:1 移植 `joinHostPort`：端口必须是纯数字才拼接。
pub fn join_host_port(host: &str, port: &str) -> String {
    let host = host.trim();
    let port = port.trim();
    if host.is_empty() {
        return String::new();
    }
    if port.is_empty() {
        return host.to_string();
    }
    if port.parse::<i64>().is_err() {
        return host.to_string();
    }
    format!("{host}:{port}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 对标 metrics_network_test.go 的噪声接口用例。
    #[test]
    fn noise_interfaces() {
        assert!(is_noise_interface("lo0"));
        assert!(is_noise_interface("awdl0"));
        assert!(is_noise_interface("utun5"));
        assert!(is_noise_interface("bridge100"));
        assert!(is_noise_interface("anpi0"));
        assert!(!is_noise_interface("en0"));
        assert!(!is_noise_interface("en5"));
    }

    /// 对标 metrics_network_test.go 的 proxy 用例。
    #[test]
    fn proxy_from_env() {
        let proxy = collect_proxy_from_env(); // 无环境变量时禁用
        if proxy.enabled {
            assert!(!proxy.host.is_empty());
        }
    }

    #[test]
    fn scutil_proxy_parse() {
        let out = "<dictionary> {\n  HTTPEnable : 1\n  HTTPPort : 7890\n  HTTPProxy : 127.0.0.1\n  SOCKSEnable : 0\n}\n";
        let proxy = collect_proxy_from_scutil_output(out);
        assert!(proxy.enabled);
        assert_eq!(proxy.kind, "HTTP");
        assert_eq!(proxy.host, "127.0.0.1:7890");
        assert_eq!(collect_proxy_from_scutil_output(""), ProxyStatus::default());
    }

    #[test]
    fn parse_proxy_host_variants() {
        assert_eq!(parse_proxy_host("http://127.0.0.1:7890"), "127.0.0.1:7890");
        assert_eq!(parse_proxy_host("socks5://u:p@1.2.3.4:1080"), "1.2.3.4:1080");
        assert_eq!(parse_proxy_host("1.2.3.4:8080"), "1.2.3.4:8080");
        assert_eq!(parse_proxy_host(""), "");
    }

    #[test]
    fn join_host_port_variants() {
        assert_eq!(join_host_port("1.2.3.4", "80"), "1.2.3.4:80");
        assert_eq!(join_host_port("1.2.3.4", ""), "1.2.3.4");
        assert_eq!(join_host_port("1.2.3.4", "abc"), "1.2.3.4");
        assert_eq!(join_host_port("", "80"), "");
    }
}
