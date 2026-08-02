//! Render a lowered [`unibind_core::ir::Interface`] into `napi-rs` binding
//! code.
//!
//! The backend targets the incumbent binding library rather than raw FFI:
//! it emits `#[napi]` wrappers around the user's functions, attaches
//! `#[napi(object)]` to record structs, converts error enums into
//! machine-decodable `napi::Error` reasons, and wraps streams and objects
//! in generated handle classes. The consuming crate therefore depends on
//! `napi` (features `napi6` + `tokio_rt`), `napi-derive`, `tokio` (features
//! `sync` + `macros`), and `unibind-runtime` directly, and builds a cdylib
//! with a `napi_build::setup()` build script.
//!
//! Everything dynamic crosses to JavaScript through the `napi::Error`
//! reason string, prefixed with `__unibind__:` (see [`error`]); the
//! generated `index.js` (`unibind-gen ts`) decodes it into real `Error`
//! subclasses, wraps stream handles into `AsyncIterable`s, and
//! materializes the enriched `.d.ts` from the embedded IR.
//!
//! Integers wider than an IEEE double holds exactly (`i64`, `u64`, `isize`,
//! `usize`) cross as JavaScript `number` in every position; `convert` owns
//! that adaptation and `mirror` the record twins it needs.

mod convert;
mod defaults;
mod error;
mod function;
mod mirror;
mod module;
mod object;
mod record;
mod stream;
mod ty;

pub use module::render;
