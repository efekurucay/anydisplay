//! Persistent host identity, paired devices, and Windows GUI connection history.
//!
//! Location: `ANYDISPLAY_STORE` if set, else `%APPDATA%\AnyDisplay\host.json` on Windows and
//! `~/.config/anydisplay/host.json` elsewhere.

use crate::proto::crypto;
use crate::util::{from_hex, to_hex};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::{fs, io};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedDevice {
    pub name: String,
    /// 32-byte `pair_secret`, hex.
    pub secret: String,
    /// ISO 8601, UTC.
    pub paired_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedConnection {
    pub address: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct File {
    host_id: String,
    #[serde(default)]
    devices: BTreeMap<String, PairedDevice>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    connections: Vec<SavedConnection>,
}

pub struct PairingStore {
    path: PathBuf,
    file: File,
}

impl PairingStore {
    pub fn default_path() -> PathBuf {
        if let Some(p) = std::env::var_os("ANYDISPLAY_STORE") {
            return PathBuf::from(p);
        }
        let base = if cfg!(windows) {
            std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            std::env::var_os("HOME")
                .map(|h| PathBuf::from(h).join(".config"))
                .unwrap_or_else(|| PathBuf::from("."))
        };
        base.join(if cfg!(windows) {
            "AnyDisplay"
        } else {
            "anydisplay"
        })
        .join("host.json")
    }

    /// Loads the store, creating a fresh host identity when the file is missing or invalid.
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<PairingStore> {
        let path = path.as_ref().to_path_buf();
        let _lock = lock_store(&path)?;
        let file = match read_store(&path) {
            Ok(file) => file,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::InvalidData
                ) =>
            {
                let file = File {
                    host_id: to_hex(&crypto::random_bytes::<16>()),
                    devices: BTreeMap::new(),
                    connections: Vec::new(),
                };
                save_store(&path, &file)?;
                file
            }
            Err(err) => return Err(err),
        };
        Ok(PairingStore { path, file })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn host_id(&self) -> [u8; 16] {
        from_hex(&self.file.host_id)
            .and_then(|b| b.try_into().ok())
            .expect("validated on load")
    }

    pub fn devices(&self) -> &BTreeMap<String, PairedDevice> {
        &self.file.devices
    }

    pub fn connections(&self) -> &[SavedConnection] {
        &self.file.connections
    }

    pub fn remember_connection(&mut self, address: &str, name: &str) -> io::Result<()> {
        self.update(|file| {
            let connections = &mut file.connections;
            let entry = match connections
                .iter()
                .position(|entry| entry.address == address)
            {
                Some(index) => connections.remove(index),
                None => SavedConnection {
                    address: address.to_owned(),
                    name: if name.trim().is_empty() {
                        address
                    } else {
                        name
                    }
                    .to_owned(),
                },
            };
            connections.insert(0, entry);
        })
    }

    pub fn edit_connection(&mut self, address: &str, name: Option<&str>) -> io::Result<()> {
        self.update(|file| {
            let connections = &mut file.connections;
            if let Some(index) = connections
                .iter()
                .position(|entry| entry.address == address)
            {
                if let Some(name) = name {
                    connections[index].name = name.to_owned();
                } else {
                    connections.remove(index);
                }
            }
        })
    }

    pub fn secret(&self, device_id: &[u8; 16]) -> Option<[u8; 32]> {
        let d = self.file.devices.get(&to_hex(device_id))?;
        from_hex(&d.secret)?.try_into().ok()
    }

    pub fn pair(
        &mut self,
        device_id: &[u8; 16],
        name: &str,
        secret: &[u8; 32],
    ) -> std::io::Result<()> {
        self.update(|file| {
            file.devices.insert(
                to_hex(device_id),
                PairedDevice {
                    name: name.to_string(),
                    secret: to_hex(secret),
                    paired_at: crate::util::iso8601_now(),
                },
            );
        })
    }

    pub fn forget(&mut self, device_id: &[u8; 16]) -> std::io::Result<()> {
        self.forget_hex(&to_hex(device_id))
    }

    pub fn forget_hex(&mut self, device_id_hex: &str) -> std::io::Result<()> {
        self.update(|file| {
            file.devices.remove(device_id_hex);
        })
    }

    pub fn forget_all(&mut self) -> std::io::Result<()> {
        self.update(|file| file.devices.clear())
    }

    fn update(&mut self, update: impl FnOnce(&mut File)) -> io::Result<()> {
        let _lock = lock_store(&self.path)?;
        let mut file = read_store(&self.path)?;
        if file.host_id != self.file.host_id {
            return Err(io::Error::other("pairing store host identity changed"));
        }
        // Sessions hold a snapshot during the handshake. Apply each change to the current file.
        update(&mut file);
        save_store(&self.path, &file)?;
        self.file = file;
        Ok(())
    }
}

fn lock_store(path: &Path) -> io::Result<fs::File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    // The lock stays on a separate file because saving replaces the store itself.
    let lock = fs::File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(append_suffix(path, ".lock"))?;
    lock.lock_exclusive()?;
    Ok(lock)
}

fn read_store(path: &Path) -> io::Result<File> {
    let file: File = serde_json::from_slice(&fs::read(path)?)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
    if from_hex(&file.host_id).is_none_or(|id| id.len() != 16) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid host identity",
        ));
    }
    Ok(file)
}

fn save_store(path: &Path, file: &File) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(file).map_err(io::Error::other)?;
    // Write-then-rename so a crash never leaves a truncated store.
    let tmp = append_suffix(path, ".tmp");
    fs::write(&tmp, json)?;
    restrict_permissions(&tmp);
    fs::rename(&tmp, path)
}

fn append_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}

/// The store holds pairing secrets. On Windows `%APPDATA%` is already private to the user.
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_named_connections_alongside_pairings() {
        let dir = std::env::temp_dir().join(format!(
            "anydisplay-store-{}",
            to_hex(&crypto::random_bytes::<8>())
        ));
        let path = dir.join("host.json");
        let lan = "192.168.1.20:47820";
        let tailscale = "100.64.0.20:48000";
        let mut store = PairingStore::open(&path).unwrap();
        let mut session = PairingStore::open(&path).unwrap();

        store.remember_connection(tailscale, "Tablet").unwrap();
        store
            .edit_connection(tailscale, Some("Tablet (away)"))
            .unwrap();
        store.remember_connection(lan, "Tablet").unwrap();
        assert_eq!(store.connections()[0].address, lan);
        session.pair(&[1; 16], "Tablet", &[2; 32]).unwrap();
        store.remember_connection(tailscale, "Device name").unwrap();

        let mut reopened = PairingStore::open(&path).unwrap();
        assert_eq!(reopened.secret(&[1; 16]), Some([2; 32]));
        assert_eq!(
            reopened.connections(),
            &[
                SavedConnection {
                    address: tailscale.into(),
                    name: "Tablet (away)".into()
                },
                SavedConnection {
                    address: lan.into(),
                    name: "Tablet".into()
                },
            ]
        );
        reopened.edit_connection(tailscale, None).unwrap();
        assert_eq!(
            PairingStore::open(&path).unwrap().connections()[0].address,
            lan
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn persists_identity_and_pairings() {
        let dir = std::env::temp_dir().join(format!(
            "anydisplay-store-{}",
            to_hex(&crypto::random_bytes::<8>())
        ));
        let path = dir.join("host.json");
        let id;
        {
            let mut s = PairingStore::open(&path).unwrap();
            id = s.host_id();
            s.pair(&[1; 16], "Tab", &[2; 32]).unwrap();
            s.pair(&[3; 16], "Other", &[4; 32]).unwrap();
            s.forget(&[3; 16]).unwrap();
        }
        let s = PairingStore::open(&path).unwrap();
        assert_eq!(s.host_id(), id);
        assert_eq!(s.secret(&[1; 16]), Some([2; 32]));
        assert_eq!(s.secret(&[3; 16]), None);
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            json["devices"]["01010101010101010101010101010101"]["name"],
            "Tab"
        );
        assert!(
            json["devices"]["01010101010101010101010101010101"]["paired_at"]
                .as_str()
                .unwrap()
                .ends_with('Z')
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn preserves_pairings_from_overlapping_sessions() {
        let dir = std::env::temp_dir().join(format!(
            "anydisplay-store-{}",
            to_hex(&crypto::random_bytes::<8>())
        ));
        let path = dir.join("host.json");
        let mut first = PairingStore::open(&path).unwrap();
        let mut second = PairingStore::open(&path).unwrap();

        first.pair(&[1; 16], "First tablet", &[11; 32]).unwrap();
        second.pair(&[2; 16], "Second tablet", &[22; 32]).unwrap();

        let saved = PairingStore::open(&path).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(saved.secret(&[1; 16]), Some([11; 32]));
        assert_eq!(saved.secret(&[2; 16]), Some([22; 32]));
    }

    #[test]
    fn pairing_does_not_restore_a_forgotten_device() {
        let dir = std::env::temp_dir().join(format!(
            "anydisplay-store-{}",
            to_hex(&crypto::random_bytes::<8>())
        ));
        let path = dir.join("host.json");
        let mut ui = PairingStore::open(&path).unwrap();
        ui.pair(&[1; 16], "First tablet", &[11; 32]).unwrap();
        let mut session = PairingStore::open(&path).unwrap();

        ui.forget_hex(&to_hex(&[1; 16])).unwrap();
        session.pair(&[2; 16], "Second tablet", &[22; 32]).unwrap();

        let saved = PairingStore::open(&path).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        assert_eq!(saved.secret(&[1; 16]), None);
        assert_eq!(saved.secret(&[2; 16]), Some([22; 32]));
    }

    #[test]
    fn reads_macos_host_layout() {
        let dir = std::env::temp_dir().join(format!(
            "anydisplay-store-{}",
            to_hex(&crypto::random_bytes::<8>())
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("host.json");
        std::fs::write(
            &path,
            r#"{"devices":{"a0a1a2a3a4a5a6a7a8a9aaabacadaeaf":{"name":"Phone","paired_at":"2026-01-02T03:04:05Z","secret":"5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a"}},"host_id":"b0b1b2b3b4b5b6b7b8b9babbbcbdbebf"}"#,
        )
        .unwrap();
        let s = PairingStore::open(&path).unwrap();
        assert_eq!(s.host_id(), std::array::from_fn(|i| 0xB0 + i as u8));
        assert_eq!(
            s.secret(&std::array::from_fn(|i| 0xA0 + i as u8)),
            Some([0x5A; 32])
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
