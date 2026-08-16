//! Render a lowered [`unibind_core::ir::Interface`] into `wasm-bindgen`
//! binding code.
//!
//! The wasm sibling of `unibind-backend-ts`: same IR, same error channel, a
//! different binding library. It emits `#[wasm_bindgen]` wrappers around the
//! user's functions, a serde twin struct per record, one generated class per
//! object and per stream export, and error conversions that carry the variant
//! identity inside a `js_sys::Error` message. The consuming crate therefore
//! depends on `wasm-bindgen`, `wasm-bindgen-futures`, `js-sys`, `serde`
//! (feature `derive`), `serde-wasm-bindgen`, `tokio` (features `sync` +
//! `macros`) and `unibind-runtime` directly, and builds a cdylib for
//! `wasm32-unknown-unknown`.
//!
//! Three policies differ from the ts backend. Each is a `wasm-bindgen` fact
//! rather than a preference:
//!
//! - **Nothing renders as an `async fn`.** `wasm-bindgen`'s support for an
//!   `async fn` taking `&self` inside an exported impl is version-dependent,
//!   so every async export renders as a *sync* fn handing back a
//!   `js_sys::Promise` built by `wasm_bindgen_futures::future_to_promise`
//!   ([`function`]). The JavaScript surface is the same object an `async fn`
//!   would have produced.
//! - **Every record crosses through a generated serde twin.** `wasm-bindgen`
//!   has no `napi(object)` analogue -- no attribute makes a plain struct cross
//!   by value -- so a record's fields reach JavaScript through `serde`, and
//!   the twin is where the boundary spellings (`f64` for a 64-bit integer, the
//!   `camelCase` key names) live without the user's own struct ever mentioning
//!   `serde` or `wasm-bindgen` ([`twin`]). Unlike the ts backend's mirror
//!   structs, the twin is unconditional: there is no "only when the spelling
//!   differs" case to detect, because no spelling crosses without it.
//! - **Structured values cross as one `JsValue`.** Numbers, strings, and
//!   whole-argument byte strings have a faithful `wasm-bindgen` ABI and cross
//!   natively; lists, maps, and records do not, so they cross as a single
//!   `JsValue` moved by `serde_wasm_bindgen` ([`ty`] owns the split).
//!
//! Integers wider than an IEEE double holds exactly (`i64`, `u64`, `isize`,
//! `usize`) cross as a JavaScript `number` in every position, checked on the
//! way in, and never as a `BigInt`: node and the browser publish one `.d.ts`
//! vocabulary, and half of it typed `bigint` would be a second one.
//! [`convert`] owns that adaptation.

mod convert;
mod defaults;
mod error;
mod function;
mod module;
mod names;
mod object;
mod record;
mod stream;
mod twin;
mod ty;

pub use module::render;
