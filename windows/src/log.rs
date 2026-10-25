//! Minimal stderr logging with a verbose switch.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static VERBOSE: AtomicBool = AtomicBool::new(false);

pub fn set_verbose(v: bool) {
    VERBOSE.store(v, Ordering::Relaxed);
}

pub fn verbose() -> bool {
    VERBOSE.load(Ordering::Relaxed)
}

pub fn write(level: &str, msg: std::fmt::Arguments<'_>) {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = t.as_secs() % 86_400;
    eprintln!(
        "{:02}:{:02}:{:02}.{:03} {level} {msg}",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60,
        t.subsec_millis()
    );
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
