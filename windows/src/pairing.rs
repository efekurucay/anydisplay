//! Persistent host identity and paired devices. Same JSON layout as the macOS host:
//! `{"host_id": hex16, "devices": {device_id_hex: {"name", "secret", "paired_at"}}}`.
//!
//! Location: `ANYDISPLAY_STORE` if set, else `%APPDATA%\AnyDisplay\host.json` on Windows and
//! `~/.config/anydisplay/host.json` elsewhere.

use crate::proto::crypto;
use crate::util::{from_hex, to_hex};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedDevice {
    pub name: String,
    /// 32-byte `pair_secret`, hex.
    pub secret: String,
    /// ISO 8601, UTC.
    pub paired_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct File {
    host_id: String,
    #[serde(default)]
    devices: BTreeMap<String, PairedDevice>,
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
        base.join(if cfg!(windows) { "AnyDisplay" } else { "anydisplay" })
            .join("host.json")
    }

    /// Loads the store, creating a fresh host identity when the file is missing or unreadable.
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<PairingStore> {
        let path = path.as_ref().to_path_buf();
        let loaded = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<File>(&b).ok())
            .filter(|f| from_hex(&f.host_id).is_some_and(|b| b.len() == 16));
        match loaded {
            Some(file) => Ok(PairingStore { path, file }),
            None => {
                let store = PairingStore {
                    path,
                    file: File {
                        host_id: to_hex(&crypto::random_bytes::<16>()),
                        devices: BTreeMap::new(),
                    },
                };
                store.save()?;
                Ok(store)
            }
        }
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
        self.file.devices.insert(
            to_hex(device_id),
            PairedDevice {
                name: name.to_string(),
                secret: to_hex(secret),
                paired_at: crate::util::iso8601_now(),
            },
        );
        self.save()
    }

    pub fn forget(&mut self, device_id: &[u8; 16]) -> std::io::Result<()> {
        self.file.devices.remove(&to_hex(device_id));
        self.save()
    }

    fn save(&self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_vec_pretty(&self.file).map_err(std::io::Error::other)?;
        // Write-then-rename so a crash never leaves a truncated store.
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json)?;
        restrict_permissions(&tmp);
        std::fs::rename(&tmp, &self.path)
    }
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
