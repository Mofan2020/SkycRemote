//! SkycRemote core.
//!
//! The pieces that every platform shares: the wire protocol, the encrypted
//! handshake, the hole that is punched through the NAT, and the capture /
//! encode / input sides of a session.

pub mod capture;
pub mod clipboard;
pub mod crypto;
pub mod encode;
pub mod files;
pub mod handshake;
pub mod hole;
pub mod host;
pub mod input;
pub mod pipeline;
pub mod power;
pub mod protocol;
pub mod session;
pub mod transfer;
pub mod transport;

/// The version of the wire protocol.  A peer that speaks a different one
/// is refused instead of being allowed to misbehave.
pub const PROTOCOL_VERSION: u16 = 3;
