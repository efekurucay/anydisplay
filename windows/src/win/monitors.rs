//! Monitor enumeration (DXGI outputs + DisplayConfig names) and display mode switching.

use super::wide;
use crate::input::Rect;
use crate::vdd;
use std::fmt;
use windows::core::{Interface, PCWSTR};
use windows::Win32::Devices::Display::*;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::Graphics::Dxgi::Common::DXGI_MODE_ROTATION;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;

pub struct Monitor {
    pub adapter: IDXGIAdapter1,
    pub output: IDXGIOutput1,
    pub adapter_name: String,
    /// GDI name, `\\.\DISPLAYn`.
    pub gdi_name: String,
    /// Desktop coordinates in physical pixels.
    pub rect: Rect,
    pub rotation: DXGI_MODE_ROTATION,
    pub primary: bool,
    pub friendly_name: String,
    pub device_path: String,
    /// The Virtual Display Driver's monitor.
    pub virtual_display: bool,
    pub hmonitor: HMONITOR,
}

// DXGI objects are free-threaded.
unsafe impl Send for Monitor {}

impl fmt::Display for Monitor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} \"{}\" {}x{} at ({},{}) on {}{}{}",
            self.gdi_name,
            self.friendly_name,
            self.rect.w,
            self.rect.h,
            self.rect.x,
            self.rect.y,
            self.adapter_name,
            if self.primary { " [primary]" } else { "" },
            if self.virtual_display {
                " [Virtual Display Driver]"
            } else {
                ""
            }
        )
    }
}

struct TargetName {
    friendly: String,
    path: String,
    edid: Option<(u16, u16)>,
}

/// GDI device name → monitor names, for active display paths.
fn target_names() -> Vec<(String, TargetName)> {
    let mut out = Vec::new();
    unsafe {
        let (mut np, mut nm) = (0u32, 0u32);
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm) != ERROR_SUCCESS {
            return out;
        }
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        if QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut np,
            paths.as_mut_ptr(),
            &mut nm,
            modes.as_mut_ptr(),
            None,
        ) != ERROR_SUCCESS
        {
            return out;
        }
        for p in &paths[..np as usize] {
            let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            src.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
            src.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
            src.header.adapterId = p.sourceInfo.adapterId;
            src.header.id = p.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut src.header) != 0 {
                continue;
            }
            let mut tgt = DISPLAYCONFIG_TARGET_DEVICE_NAME::default();
            tgt.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
            tgt.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
            tgt.header.adapterId = p.targetInfo.adapterId;
            tgt.header.id = p.targetInfo.id;
            if DisplayConfigGetDeviceInfo(&mut tgt.header) != 0 {
                continue;
            }
            // DISPLAYCONFIG_TARGET_DEVICE_NAME_FLAGS bit 1: edidIdsValid.
            let edid_valid = tgt.flags.Anonymous.value & 0x2 != 0;
            out.push((
                wide(&src.viewGdiDeviceName),
                TargetName {
                    friendly: wide(&tgt.monitorFriendlyDeviceName),
                    path: wide(&tgt.monitorDevicePath),
                    edid: edid_valid.then_some((tgt.edidManufactureId, tgt.edidProductCodeId)),
                },
            ));
        }
    }
    out
}

/// Every output attached to the desktop, in DXGI order (the `--monitor` index).
pub fn list() -> anyhow::Result<Vec<Monitor>> {
    let names = target_names();
    let mut out = Vec::new();
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            a += 1;
            let adapter_name = adapter
                .GetDesc1()
                .map(|d| wide(&d.Description))
                .unwrap_or_default();
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                o += 1;
                let Ok(desc) = output.GetDesc() else { continue };
                if !desc.AttachedToDesktop.as_bool() {
                    continue;
                }
                let Ok(output1) = output.cast::<IDXGIOutput1>() else {
                    continue;
                };
                let gdi_name = wide(&desc.DeviceName);
                let r = desc.DesktopCoordinates;
                let mut mi = MONITORINFO {
                    cbSize: size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                let primary = GetMonitorInfoW(desc.Monitor, &mut mi).as_bool()
                    && mi.dwFlags & windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY
                        != 0;
                let t = names.iter().find(|(n, _)| *n == gdi_name).map(|(_, t)| t);
                let (friendly, path, edid) = t
                    .map(|t| (t.friendly.clone(), t.path.clone(), t.edid))
                    .unwrap_or_default();
                out.push(Monitor {
                    adapter: adapter.clone(),
                    output: output1,
                    adapter_name: adapter_name.clone(),
                    virtual_display: vdd::is_vdd_monitor(&friendly, &path, edid),
                    gdi_name,
                    rect: Rect {
                        x: r.left,
                        y: r.top,
                        w: r.right - r.left,
                        h: r.bottom - r.top,
                    },
                    rotation: desc.Rotation,
                    primary,
                    friendly_name: friendly,
                    device_path: path,
                    hmonitor: desc.Monitor,
                });
            }
        }
    }
    Ok(out)
}

/// Bounds of the whole virtual screen (for `MOUSEEVENTF_VIRTUALDESK`).
pub fn virtual_screen() -> Rect {
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        Rect {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            w: GetSystemMetrics(SM_CXVIRTUALSCREEN),
            h: GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Switches `gdi_name` to `w x h` (preferring `hz`) without saving it to the registry, if the
/// driver offers such a mode. Returns true on success; [`restore_mode`] goes back to the registry mode.
pub fn set_mode(gdi_name: &str, w: u32, h: u32, hz: u32) -> bool {
    let name = to_wide(gdi_name);
    let mut best: Option<DEVMODEW> = None;
    unsafe {
        let mut i = 0;
        loop {
            let mut dm = DEVMODEW {
                dmSize: size_of::<DEVMODEW>() as u16,
                ..Default::default()
            };
            if !EnumDisplaySettingsW(
                PCWSTR(name.as_ptr()),
                ENUM_DISPLAY_SETTINGS_MODE(i),
                &mut dm,
            )
            .as_bool()
            {
                break;
            }
            i += 1;
            if dm.dmPelsWidth == w && dm.dmPelsHeight == h {
                let better = best.is_none_or(|b| {
                    (dm.dmDisplayFrequency as i64 - hz as i64).abs()
                        < (b.dmDisplayFrequency as i64 - hz as i64).abs()
                });
                if better {
                    best = Some(dm);
                }
            }
        }
        let Some(mut dm) = best else { return false };
        dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY;
        ChangeDisplaySettingsExW(PCWSTR(name.as_ptr()), Some(&dm), None, CDS_TYPE(0), None)
            == DISP_CHANGE_SUCCESSFUL
    }
}

pub fn restore_mode(gdi_name: &str) {
    let name = to_wide(gdi_name);
    unsafe {
        let _ = ChangeDisplaySettingsExW(PCWSTR(name.as_ptr()), None, None, CDS_TYPE(0), None);
    }
}
