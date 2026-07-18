//! Discover Tasmota devices by probing a range of hosts over HTTP.
//!
//! No addresses are hardcoded: the range is taken from `--range`, or derived from
//! the host's own primary IPv4 (assuming a `/24`). A host is Tasmota when its
//! `Status 0` response carries a firmware version (see [`crate::parse`]).

use std::net::{Ipv4Addr, UdpSocket};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::error::{Error, Result};
use crate::model::DeviceStatus;
use crate::ops;
use crate::parse::looks_like_tasmota;
use crate::transport::{Credentials, DeviceAddr, Transport};

/// A device found during a scan.
#[derive(Debug, Clone)]
pub struct Discovered {
    pub host: String,
    pub status: DeviceStatus,
}

/// Expand an IPv4 CIDR (e.g. `192.0.2.0/24`) into candidate host addresses,
/// excluding the network and broadcast addresses. Bounded to `/16` to avoid
/// accidentally enumerating millions of hosts.
pub fn hosts_in_cidr(cidr: &str) -> Result<Vec<String>> {
    let (addr, prefix) = cidr.split_once('/').ok_or_else(|| Error::Usage {
        message: format!("invalid CIDR `{cidr}` (expected e.g. 192.0.2.0/24)"),
    })?;
    let base: Ipv4Addr = addr.parse().map_err(|_| Error::Usage {
        message: format!("invalid IPv4 address in CIDR `{cidr}`"),
    })?;
    let prefix: u32 = prefix.parse().map_err(|_| Error::Usage {
        message: format!("invalid prefix length in CIDR `{cidr}`"),
    })?;
    if prefix > 32 {
        return Err(Error::Usage {
            message: format!("prefix /{prefix} out of range"),
        });
    }
    if prefix < 16 {
        return Err(Error::Usage {
            message: format!("refusing to scan a range larger than /16 (got /{prefix})"),
        });
    }

    let base_u32 = u32::from(base);
    let host_bits = 32 - prefix;
    // host_bits is 0..=16 (prefix is bounded to 16..=32 above), so this never
    // overflows. For /32 this yields an all-ones mask, keeping the single host.
    let mask = !0u32 << host_bits;
    let network = base_u32 & mask;
    let count = 1u64 << host_bits;

    let mut hosts = Vec::new();
    if count <= 2 {
        // /31 and /32: no network/broadcast convention, use all addresses.
        for i in 0..count {
            hosts.push(Ipv4Addr::from(network + i as u32).to_string());
        }
    } else {
        for i in 1..(count - 1) {
            hosts.push(Ipv4Addr::from(network + i as u32).to_string());
        }
    }
    Ok(hosts)
}

/// Best-effort detection of the host's primary IPv4, used to derive a default
/// `/24` scan range. Uses the UDP-connect trick (no packets are sent) against a
/// documentation address so nothing real is contacted.
pub fn detect_local_cidr() -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:80").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    match ip {
        std::net::IpAddr::V4(v4) => {
            let o = v4.octets();
            Some(format!("{}.{}.{}.0/24", o[0], o[1], o[2]))
        }
        _ => None,
    }
}

/// Probe every host concurrently and return those that answer as Tasmota.
///
/// `concurrency` caps the number of in-flight probes. Unreachable hosts are
/// silently skipped (a scan expects most addresses to be empty).
pub fn scan<T: Transport + Sync>(
    transport: &T,
    hosts: &[String],
    concurrency: usize,
    credentials: Option<&Credentials>,
) -> Vec<Discovered> {
    let next = AtomicUsize::new(0);
    let found: Mutex<Vec<Discovered>> = Mutex::new(Vec::new());
    let workers = concurrency.max(1).min(hosts.len().max(1));

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let idx = next.fetch_add(1, Ordering::Relaxed);
                    if idx >= hosts.len() {
                        break;
                    }
                    let host = &hosts[idx];
                    let addr = DeviceAddr::new(host.clone()).with_credentials(credentials.cloned());
                    // A probe only needs Status 0; ignore anything that fails or
                    // does not look like Tasmota.
                    if let Ok(value) = transport.command(&addr, "Status 0")
                        && looks_like_tasmota(&value)
                        && let Ok(status) = ops::status_from_value(transport, &addr, &value)
                    {
                        found.lock().unwrap().push(Discovered {
                            host: host.clone(),
                            status,
                        });
                    }
                }
            });
        }
    });

    let mut result = found.into_inner().unwrap();
    result.sort_by(|a, b| a.status.display_name().cmp(b.status.display_name()));
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cidr_24_excludes_network_and_broadcast() {
        let hosts = hosts_in_cidr("192.0.2.0/24").unwrap();
        assert_eq!(hosts.len(), 254);
        assert_eq!(hosts[0], "192.0.2.1");
        assert_eq!(hosts[253], "192.0.2.254");
    }

    #[test]
    fn cidr_rejects_too_large_and_malformed() {
        assert!(hosts_in_cidr("192.0.2.0/8").is_err());
        assert!(hosts_in_cidr("not-a-cidr").is_err());
        assert!(hosts_in_cidr("192.0.2.0/33").is_err());
    }

    #[test]
    fn cidr_32_is_single_host() {
        let hosts = hosts_in_cidr("198.51.100.7/32").unwrap();
        assert_eq!(hosts, vec!["198.51.100.7"]);
    }
}
