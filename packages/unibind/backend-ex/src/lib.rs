//! Render a lowered [`unibind_core::ir::Interface`] into `rustler` binding
//! code and the Elixir modules that call it.
//!
//! The backend targets the incumbent binding library rather than raw FFI:
//! it emits `#[rustler::nif]` wrappers around the user's functions, derives
//! `NifStruct` onto record structs, registers objects as BEAM resources,
//! and hands async functions and streams to `unibind-ex-runtime`. The
//! consuming crate therefore depends on `rustler` and `unibind-ex-runtime`
//! directly and builds a `cdylib` that `:erlang.load_nif/2` loads. The
//! matching Elixir side (`<Ns>.Native` stubs and the typespec'd `<Ns>`
//! wrapper) comes from [`host_modules`], which `unibind-gen`'s `ExEmitter`
//! writes to disk.

mod error;
mod function;
mod host;
mod module;
mod names;
mod object;
mod record;
mod ty;

pub use host::{HostModules, host_modules};
pub use module::render;
