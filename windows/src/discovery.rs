//! Wi-Fi discovery: browse `_usethatscreen._tcp` (PROTOCOL.md 2).

use mdns_sd::{ServiceDaemon, ServiceEvent};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

pub const SERVICE_TYPE: &str = "_usethatscreen._tcp.local.";

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
            let dev = device_from_info(&info);
            found.retain(|d| d.name != dev.name);
            found.push(dev);
        }
    }
    let _ = daemon.shutdown();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(found)
}

fn device_from_info(info: &mdns_sd::ResolvedService) -> NetworkDevice {
    let name = instance_name(info.get_fullname());
    let mut addrs: Vec<SocketAddr> = info
        .get_addresses()
        .iter()
        .map(|a| SocketAddr::new(a.to_ip_addr(), info.get_port()))
        .collect();
    // IPv4 first: link-local IPv6 needs a scope id the user cannot type.
    addrs.sort_by_key(|a| (!a.is_ipv4(), a.to_string()));
    NetworkDevice {
        name,
        device_id: info.get_property_val_str("id").map(str::to_string),
        version: info.get_property_val_str("v").map(str::to_string),
        addrs,
    }
}

/// Continuously monitors `_usethatscreen._tcp` on a background thread and keeps a live device list.
pub struct WifiMonitor {
    daemon: ServiceDaemon,
    devices: std::sync::Arc<std::sync::Mutex<Vec<NetworkDevice>>>,
}

impl WifiMonitor {
    pub fn start() -> anyhow::Result<Self> {
        let daemon = ServiceDaemon::new()?;
        let rx = daemon.browse(SERVICE_TYPE)?;
        let devices = std::sync::Arc::new(std::sync::Mutex::new(Vec::<NetworkDevice>::new()));
        let shared = devices.clone();
        std::thread::Builder::new()
            .name("usethatscreen-mdns".into())
            .spawn(move || {
                while let Ok(event) = rx.recv() {
                    match event {
                        ServiceEvent::ServiceResolved(info) => {
                            let dev = device_from_info(&info);
                            if let Ok(mut list) = shared.lock() {
                                list.retain(|d| d.name != dev.name);
                                list.push(dev);
                                list.sort_by(|a, b| a.name.cmp(&b.name));
                            }
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => {
                            let name = instance_name(&fullname);
                            if let Ok(mut list) = shared.lock() {
                                list.retain(|d| d.name != name);
                            }
                        }
                        _ => {}
                    }
                }
            })?;
        Ok(Self { daemon, devices })
    }

    pub fn devices(&self) -> Vec<NetworkDevice> {
        self.devices.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

impl Drop for WifiMonitor {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}

/// `My Tablet._usethatscreen._tcp.local.` → `My Tablet`.
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
            super::instance_name("My Tablet._usethatscreen._tcp.local."),
            "My Tablet"
        );
        assert_eq!(super::instance_name("odd"), "odd");
    }
}
