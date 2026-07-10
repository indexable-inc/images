//! Minecraft Java Edition wire-protocol primitives.
//!
//! The pieces of the protocol this repo actually speaks: `VarInt` encoding and
//! packet framing ([`varint`]), length-prefixed strings ([`wire`]), the
//! Server List Ping status exchange ([`slp`]), and legacy format-code
//! stripping for MOTD comparison ([`text`]). mc-probe
//! (packages/minecraft/minecraft/probe) is the primary consumer, through the
//! unibind Python bindings in `py/`; mc-bot
//! (packages/minecraft/minecraft/bot) builds its packet layer on the same
//! primitives. Keeping the protocol logic here means one implementation
//! serves every language a health check or test harness is written in.

pub mod slp;
pub mod text;
pub mod varint;
pub mod wire;

pub use slp::{DEFAULT_PORT, ServerAddress, SlpStatus, query};
pub use text::strip_format_codes;
