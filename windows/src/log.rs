//! Minimal stderr logging with a verbose switch and an in-memory ring buffer for the GUI.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_RECENT: usize = 250;

static VERBOSE: AtomicBool = AtomicBool::new(false);
static RECENT: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();

fn ring() -> &'static Mutex<VecDeque<String>> {
    RECENT.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_RECENT)))
}

pub fn set_verbose(v: bool) {
    VERBOSE.store(v, Ordering::Relaxed);
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

pub fn recent() -> Vec<String> {
    ring().lock().unwrap().iter().cloned().collect()
}

pub fn clear_recent() {
    ring().lock().unwrap().clear();
}

pub fn write(level: &str, msg: std::fmt::Arguments<'_>) {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = t.as_secs() % 86_400;
    let line = format!(
        "{:02}:{:02}:{:02}.{:03} {level} {msg}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60,
        t.subsec_millis()
    );
    eprintln!("{line}");
    if let Ok(mut buf) = ring().lock() {
        if buf.len() >= MAX_RECENT {
            buf.pop_front();
        }
        buf.push_back(line);
    }
}

#[macro_export]
macro_rules! info {
    ($($t:tt)*) => { $crate::log::write("INFO ", format_args!($($t)*)) };
}

#[macro_export]
macro_rules! warn {
    ($($t:tt)*) => { $crate::log::write("WARN ", format_args!($($t)*)) };
}

#[macro_export]
macro_rules! debug {
    ($($t:tt)*) => { if $crate::log::verbose() { $crate::log::write("DEBUG", format_args!($($t)*)) } };
}
