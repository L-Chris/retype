//! Endpoints are routing hints. Device identity is authenticated independently.
use crate::Result;
use std::net::{IpAddr, SocketAddr, ToSocketAddrs};

pub const DEFAULT_PORT: u16 = 50206;

pub fn allowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            v.is_private()
                || v.is_link_local()
                || v.is_loopback()
                || (v.octets()[0] == 100 && (64..=127).contains(&v.octets()[1]))
        }
        IpAddr::V6(v) => v.is_loopback() || v.is_unique_local() || v.is_unicast_link_local(),
    }
}

pub fn normalize_address(value: &str) -> Result<String> {
    let value = value.trim();
    if value.len() > 300 || value.is_empty() {
        return Err("请输入设备 IP 或域名，可附带端口".into());
    }
    let url = url::Url::parse(&format!("tcp://{value}")).map_err(|_| "设备地址格式无效")?;
    if !url.username().is_empty()
        || url.password().is_some()
        || !url.path().is_empty()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("只需填写 IP 或域名和端口".into());
    }
    let host = url.host_str().ok_or("缺少设备地址")?.to_ascii_lowercase();
    let port = url.port().unwrap_or(DEFAULT_PORT);
    if port == 0 {
        return Err("端口范围为 1–65535".into());
    }
    if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
        if !allowed(ip) {
            return Err("仅支持局域网或 Tailscale 地址".into());
        }
        return Ok(SocketAddr::new(ip, port).to_string());
    }
    if host.len() > 253
        || !host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
    {
        return Err("设备域名格式无效".into());
    }
    Ok(format!("{host}:{port}"))
}

pub fn resolve(value: &str) -> Result<Vec<SocketAddr>> {
    let value = normalize_address(value)?;
    let mut addresses: Vec<_> = value
        .to_socket_addrs()
        .map_err(|_| "设备域名解析失败")?
        .filter(|a| allowed(a.ip()))
        .take(16)
        .collect();
    addresses.sort_by_key(|a| (is_tailscale(a.ip()), a.is_ipv6()));
    addresses.dedup();
    if addresses.is_empty() {
        return Err("设备地址不属于局域网或 Tailscale".into());
    }
    Ok(addresses)
}
pub fn is_tailscale(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.octets()[0] == 100 && (64..=127).contains(&v.octets()[1]),
        IpAddr::V6(v) => v.segments()[0..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}

/// Physical LAN addresses first, DNS hints next, overlay addresses last.
pub fn priority(value: &str) -> Result<u8> {
    let normalized = normalize_address(value)?;
    let url = url::Url::parse(&format!("tcp://{normalized}")).map_err(|_| "设备地址格式无效")?;
    Ok(
        match url
            .host_str()
            .unwrap_or("")
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
        {
            Ok(ip) => {
                if is_tailscale(ip) {
                    2
                } else {
                    0
                }
            }
            Err(_) => 1,
        },
    )
}
pub fn normalize_list(value: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for item in value
        .split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let address = normalize_address(item)?;
        if !out.contains(&address) {
            out.push(address);
        }
        if out.len() > 8 {
            return Err("最多保存 8 个连接地址".into());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoints_accept_exact_overlay_range_and_reject_public_or_url_parts() {
        for a in [
            "100.64.0.1",
            "100.127.255.254",
            "192.168.31.128",
            "home",
            "home.tailnet.ts.net",
        ] {
            assert!(normalize_address(a).is_ok(), "{a}");
        }
        assert_eq!(
            normalize_address(" home:50206 ").ok().as_deref(),
            Some("home:50206")
        );
        assert_eq!(
            normalize_address("[fd7a:115c:a1e0::1]:50206")
                .ok()
                .as_deref(),
            Some("[fd7a:115c:a1e0::1]:50206")
        );
        for a in [
            "100.63.255.255",
            "100.128.0.1",
            "8.8.8.8",
            "home:0",
            "home:65536",
            "home/path",
            "user@home",
            "home?x=1",
            "https://home",
        ] {
            assert!(normalize_address(a).is_err(), "{a}");
        }
    }
}
