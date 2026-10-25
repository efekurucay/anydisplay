//! USB transport: `adb forward tcp:<local> tcp:47820` (PROTOCOL.md 2).

use std::path::PathBuf;
use std::process::Command;

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
        let out = self.run(&[
            "-s",
            serial,
            "forward",
            "tcp:0",
            &format!("tcp:{remote_port}"),
        ])?;
        out.trim().parse().map_err(|_| {
            std::io::Error::other(format!("unexpected adb forward output {:?}", out.trim()))
        })
    }

    pub fn remove_forward(&self, serial: &str, local_port: u16) {
        let _ = self.run(&[
            "-s",
            serial,
            "forward",
            "--remove",
            &format!("tcp:{local_port}"),
        ]);
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

/// Parses `adb devices -l`, keeping devices in the `device` state (online and authorized).
pub fn parse_devices(output: &str) -> Vec<UsbDevice> {
    output
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 2 || parts[1] != "device" {
                return None;
            }
            let model = parts[2..]
                .iter()
                .find_map(|p| p.strip_prefix("model:"))
                .map(str::to_string);
            Some(UsbDevice {
                serial: parts[0].to_string(),
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
}
