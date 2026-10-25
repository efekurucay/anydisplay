//! Two-way `text/plain` clipboard (PROTOCOL.md 5.1 CLIPBOARD). Changes are detected with the
//! clipboard sequence number; only changes made during the session are sent.

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, PeekMessageW, HWND_MESSAGE, MSG, PM_REMOVE,
    WINDOW_EX_STYLE, WINDOW_STYLE,
};

pub struct Clipboard {
    /// Message-only window: SetClipboardData needs an owner after EmptyClipboard.
    hwnd: HWND,
    last_seq: u32,
}

impl Clipboard {
    pub fn new() -> anyhow::Result<Clipboard> {
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("AnyDisplay clipboard"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }?;
        Ok(Clipboard {
            hwnd,
            last_seq: unsafe { GetClipboardSequenceNumber() },
        })
    }

    /// Handles messages for the clipboard window. Other applications' EmptyClipboard sends the
    /// owner WM_DESTROYCLIPBOARD and waits for it, so this runs on every session tick.
    pub fn pump(&self) {
        let mut msg = MSG::default();
        unsafe {
            while PeekMessageW(&mut msg, Some(self.hwnd), 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&msg);
            }
        }
    }

    fn open(&self) -> bool {
        // Another application may hold the clipboard for a moment.
        for _ in 0..10 {
            if unsafe { OpenClipboard(Some(self.hwnd)) }.is_ok() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    }

    pub fn changed(&mut self) -> Option<String> {
        let seq = unsafe { GetClipboardSequenceNumber() };
        if seq == self.last_seq {
            return None;
        }
        self.last_seq = seq;
        if !self.open() {
            return None;
        }
        let text = unsafe {
            GetClipboardData(CF_UNICODETEXT.0 as u32)
                .ok()
                .and_then(|h| {
                    let g = HGLOBAL(h.0);
                    let p = GlobalLock(g) as *const u16;
                    if p.is_null() {
                        return None;
                    }
                    let s = PCWSTR(p).to_string().ok();
                    let _ = GlobalUnlock(g);
                    s
                })
        };
        unsafe {
            let _ = CloseClipboard();
        }
        text
    }

    pub fn set(&mut self, text: &str) {
        let units: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        if !self.open() {
            return;
        }
        unsafe {
            if EmptyClipboard().is_ok() {
                if let Ok(g) = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2) {
                    let p = GlobalLock(g) as *mut u16;
                    if !p.is_null() {
                        std::ptr::copy_nonoverlapping(units.as_ptr(), p, units.len());
                        let _ = GlobalUnlock(g);
                        // On success the clipboard owns the memory.
                        let _ = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(g.0)));
                    }
                }
            }
            let _ = CloseClipboard();
            // Our own change is not echoed back to the client.
            self.last_seq = GetClipboardSequenceNumber();
        }
    }
}

impl Drop for Clipboard {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
