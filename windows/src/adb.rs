//! USB transport: `adb forward tcp:<local> tcp:47820` (PROTOCOL.md 2).

use std::path::PathBuf;
use std::process::Command;

/// What `adb devices` prints for a device that reports an empty serial. `adb -s` cannot select it.
const NO_SERIAL: &str = "(no serial number)";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbDevice {
    pub serial: String,
    pub model: Option<String>,
}

pub struct Adb {
    pub path: PathBuf,
}

impl Adb {
    /// `ANYDISPLAY_ADB`, then `PATH`, then the Android SDK default locations.
    pub fn locate() -> Option<Adb> {
        let exe = if cfg!(windows) { "adb.exe" } else { "adb" };
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(p) = std::env::var_os("ANYDISPLAY_ADB") {
            candidates.push(p.into());
        }
        if let Some(path) = std::env::var_os("PATH") {
            candidates.extend(std::env::split_paths(&path).map(|d| d.join(exe)));
        }
        for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
            if let Some(sdk) = std::env::var_os(var) {
                candidates.push(PathBuf::from(sdk).join("platform-tools").join(exe));
            }
        }
        if cfg!(windows) {
            if let Some(local) = std::env::var_os("LOCALAPPDATA") {
                candidates.push(PathBuf::from(local).join(r"Android\Sdk\platform-tools\adb.exe"));
            }
        } else if let Some(home) = std::env::var_os("HOME") {
            candidates.push(PathBuf::from(&home).join("Library/Android/sdk/platform-tools/adb"));
            candidates.push(PathBuf::from(home).join("Android/Sdk/platform-tools/adb"));
        }
        candidates
            .into_iter()
            .find(|p| p.is_file())
            .map(|path| Adb { path })
    }

    pub fn devices(&self) -> std::io::Result<Vec<UsbDevice>> {
        Ok(parse_devices(&self.run(&["devices", "-l"])?))
    }

    /// Forwards a free local port to `remote_port` on the device and returns the local port.
    pub fn forward(&self, serial: &str, remote_port: u16) -> std::io::Result<u16> {
        let remote = format!("tcp:{remote_port}");
        let out = self.run(&targeted(serial, &["forward", "tcp:0", &remote]))?;
        out.trim().parse().map_err(|_| {
            std::io::Error::other(format!("unexpected adb forward output {:?}", out.trim()))
        })
    }

    pub fn remove_forward(&self, serial: &str, local_port: u16) {
        let local = format!("tcp:{local_port}");
        let _ = self.run(&targeted(serial, &["forward", "--remove", &local]));
    }

    fn run(&self, args: &[&str]) -> std::io::Result<String> {
        let mut cmd = Command::new(&self.path);
        cmd.args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = cmd.output()?;
        if !out.status.success() {
            return Err(std::io::Error::other(format!(
                "adb {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// Selects a serial explicitly, or the sole USB device when no serial is available.
fn targeted<'a>(serial: &'a str, args: &[&'a str]) -> Vec<&'a str> {
    let mut all = if serial == NO_SERIAL {
        // Select USB explicitly so ANDROID_SERIAL cannot redirect this request.
        vec!["-d"]
    } else {
        vec!["-s", serial]
    };
    all.extend_from_slice(args);
    all
}

/// Parses `adb devices -l`, keeping devices in the `device` state (online and authorized).
pub fn parse_devices(output: &str) -> Vec<UsbDevice> {
    output
        .lines()
        .filter_map(|line| {
            let after_no_serial = line.strip_prefix(NO_SERIAL);
            let parts: Vec<&str> = after_no_serial.unwrap_or(line).split_whitespace().collect();
            let (serial, fields) = match after_no_serial {
                Some(_) => (NO_SERIAL, &parts[..]),
                None => (*parts.first()?, &parts[1..]),
            };
            if fields.first() != Some(&"device") {
                return None;
            }
            let model = fields[1..]
                .iter()
                .find_map(|p| p.strip_prefix("model:"))
                .map(str::to_string);
            Some(UsbDevice {
                serial: serial.to_string(),
                model,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_device_list() {
        let out = "List of devices attached\n\
                   R58N12345      device usb:1-1 product:gts7 model:SM_T870 device:gts7 transport_id:3\n\
                   emulator-5554  offline\n\
                   ZY22           unauthorized usb:1-2 transport_id:4\n\
                   0123           device transport_id:5\n\n";
        assert_eq!(
            parse_devices(out),
            vec![
                UsbDevice {
                    serial: "R58N12345".into(),
                    model: Some("SM_T870".into())
                },
                UsbDevice {
                    serial: "0123".into(),
                    model: None
                },
            ]
        );
    }

    #[test]
    fn parses_device_without_serial() {
        let out = "List of devices attached\r\n\
                   (no serial number)     device product:9469X_RU model:9469X device:Bellona_WF_GL transport_id:1\r\n\
                   (no serial number)     unauthorized transport_id:2\r\n\r\n";
        assert_eq!(
            parse_devices(out),
            vec![UsbDevice {
                serial: NO_SERIAL.into(),
                model: Some("9469X".into())
            }]
        );
    }

    #[test]
    fn selects_serial_or_usb_transport() {
        assert_eq!(
            targeted("R58N12345", &["forward", "tcp:0"]),
            ["-s", "R58N12345", "forward", "tcp:0"]
        );
        assert_eq!(
            targeted(NO_SERIAL, &["forward", "tcp:0"]),
            ["-d", "forward", "tcp:0"]
        );
    }

    #[test]
    fn removal_uses_the_same_usb_selector() {
        assert_eq!(
            targeted(NO_SERIAL, &["forward", "--remove", "tcp:12345"]),
            ["-d", "forward", "--remove", "tcp:12345"]
        );
        assert_eq!(
            targeted("known", &["forward", "--remove", "tcp:12345"]),
            ["-s", "known", "forward", "--remove", "tcp:12345"]
        );
    }
}
