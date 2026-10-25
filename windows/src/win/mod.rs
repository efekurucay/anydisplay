//! Windows platform: DXGI Desktop Duplication capture, Media Foundation H.264/HEVC encoding,
//! synthetic pointer + SendInput injection, WASAPI loopback audio, clipboard.

pub mod audio;
pub mod capture;
pub mod clipboard;
pub mod encoder;
pub mod input;
pub mod monitors;
pub mod platform;

pub use platform::WinPlatform;

use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};

/// Per-monitor DPI awareness, so DXGI desktop coordinates, pointer injection and SendInput all
/// use physical pixels.
pub fn init_process() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// Joins the COM multithreaded apartment for the current thread. All COM / Media Foundation
/// objects in this crate live in the MTA, which is why they may move between threads.
pub struct ComGuard(bool);

impl ComGuard {
    pub fn new() -> ComGuard {
        ComGuard(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Default for ComGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

/// Null-terminated UTF-16 buffer → String.
pub(crate) fn wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}
