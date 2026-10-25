//! Platform-independent implementation of docs/PROTOCOL.md (host role).

pub mod crypto;
pub mod frame;
pub mod handshake;
pub mod messages;
pub mod wire;

pub use frame::{Cipher, Frame};
pub use messages::*;
