//! AnyDisplay Windows host. Everything outside `win` is platform-independent and tested on any OS.

#[macro_use]
pub mod log;

pub mod adb;
pub mod annexb;
pub mod audio_convert;
pub mod cursor;
pub mod discovery;
pub mod input;
pub mod keymap;
pub mod negotiate;
pub mod pairing;
pub mod platform;
pub mod proto;
pub mod session;
pub mod testsrc;
pub mod util;
pub mod vdd;

#[cfg(windows)]
pub mod win;
