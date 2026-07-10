//! Public Rust SDK for ix.
//!
//! This crate is the public face of the SDK. It compiles and links against the
//! private `ix-sdk-wire` crate WITHOUT carrying its source: the nix build
//! injects the prebuilt `ix-sdk-wire` rlib+rmeta (fetched from R2) over a
//! metadata-faithful stub, so this crate typechecks against the prebuilt rmeta
//! and links the prebuilt rlib. See `packages/sdk/rust/build.nix`.

// Re-export the wire surface so SDK consumers get the boundary types through
// `ix_sdk::*`. This `use` is the typecheck against the prebuilt rmeta: the
// symbols must exist in the injected rlib for the SDK to compile and link.
pub use ix_sdk_wire::{
    DecodeError, Decoder, Encoder, IX_BUF_VERSION, IxBuf, IxError, IxErrorCode, IxErrorKind,
};

/// Round-trip an error code through the prebuilt wire crate.
///
/// Calls into `ix-sdk-wire`'s `from_u32` / `as_u32`, which only resolve if the
/// SDK linked the prebuilt rlib (not stub source, which has no such items).
/// `0` is the reserved `Unknown` sentinel, so this always returns `0`.
#[must_use]
pub fn normalize_error_code(raw: u32) -> u32 {
    IxErrorCode::from_u32(raw).as_u32()
}
