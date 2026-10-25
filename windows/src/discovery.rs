//! Wi-Fi discovery: browse `_anydisplay._tcp` (PROTOCOL.md 2).

use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

pub const SERVICE_TYPE: &str = "_anydisplay._tcp.local.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkDevice {
    /// DNS-SD instance name: the device name.
    pub name: String,
    /// TXT `id`: 32 hex chars.
    pub device_id: Option<String>,
    pub version: Option<String>,
    pub addrs: Vec<SocketAddr>,
}

/// Browses for `duration` and returns the resolved clients, sorted by name.
pub fn browse(duration: Duration) -> anyhow::Result<Vec<NetworkDevice>> {
    let daemon = ServiceDaemon::new()?;
    let rx = daemon.browse(SERVICE_TYPE)?;
    let deadline = Instant::now() + duration;
    let mut found: Vec<NetworkDevice> = Vec::new();
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        let Ok(event) = rx.recv_timeout(left) else {
            break;
        };
        if let ServiceEvent::ServiceResolved(info) = event {
            let name = instance_name(info.get_fullname());
            let mut addrs: Vec<SocketAddr> = info
                .get_addresses()
                .iter()
                .map(|a| SocketAddr::new(a.to_ip_addr(), info.get_port()))
                .collect();
            // IPv4 first: link-local IPv6 needs a scope id the user cannot type.
            addrs.sort_by_key(|a| (!a.is_ipv4(), a.to_string()));
            let dev = NetworkDevice {
                name,
                device_id: info.get_property_val_str("id").map(str::to_string),
                version: info.get_property_val_str("v").map(str::to_string),
                addrs,
            };
            found.retain(|d| d.name != dev.name);
            found.push(dev);
        }
    }
    let _ = daemon.shutdown();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

/// `My Tablet._anydisplay._tcp.local.` → `My Tablet`.
pub fn instance_name(fullname: &str) -> String {
    fullname
        .strip_suffix(&format!(".{SERVICE_TYPE}"))
        .unwrap_or(fullname)
        .to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn instance_names() {
        assert_eq!(
            super::instance_name("My Tablet._anydisplay._tcp.local."),
            "My Tablet"
        );
        assert_eq!(super::instance_name("odd"), "odd");
    }
}
