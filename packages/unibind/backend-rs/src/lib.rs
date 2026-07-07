//! Render a lowered [`unibind_core::ir::Interface`] into a Rust client over
//! a stable ABI.
//!
//! Two surfaces come out of one IR. [`render`] emits the *engine* half: a
//! hidden module the `#[unibind::export]` macro splices into the annotated
//! crate (under the `rs` cargo feature) with `#[stabby::export]`ed
//! `extern "C"` wrappers, ABI-stable mirror structs, and an IR-hash
//! handshake symbol. [`render_client`] emits the *consumer* half: the source
//! files of a standalone `<name>-client` crate that dlopens the engine
//! cdylib, verifies the handshake, and wraps every export in safe idiomatic
//! Rust, including real futures whose `Drop` cancels the engine-side future.
//!
//! # Why stabby and not crABI
//!
//! crABI (`extern "crabi"`, RFC PR 3470) is an accepted rust-lang
//! compiler-team experiment (MCP 631, May 2023) to define an interoperable
//! stable ABI for Rust, but as of mid-2026 it remains unimplemented: the
//! experimental feature-gate PR (rust-lang/rust#105586) has never merged,
//! the tracking issue (rust-lang/rust#111423) shows no completed steps since
//! 2023, and no `crabi` feature gate exists in nightly rustc. The experiment
//! is stalled rather than formally abandoned; there is no usable
//! `extern "crabi"` today, which is why this backend pins its ABI with
//! stabby instead.
//!
//! stabby is pinned at `>=72.1.8`: 72.1.8 fixes the `&mut dyn` vtable bug
//! (ZettaScaleLabs/stabby#130) and ships deterministic macro expansion, which
//! reproducible builds need.
//!
//! # The futures verdict
//!
//! Real stable futures shipped; the documented fallback (sync calls plus a
//! callback vtable) was not needed. An exported `async fn` crosses as
//! `stabby::future::DynFuture<'static, Output>`, a `dynptr` box whose
//! vtable carries `poll` and `drop` shims compiled into the engine. The
//! client side gets a plain `impl core::future::Future`, so `.await` works,
//! and dropping the client future runs the engine's drop shim through the
//! vtable, cancelling the engine-side future: that is the cancellation
//! mechanism, proven by the conformance suite's `hang_until_dropped`
//! witness. Wakers cross through stabby's safe `StableWaker`, which
//! allocates per waker clone; acceptable for a binding boundary, and the
//! `stabby_unsafe_wakers` cfg stays off because a mismatched waker ABI
//! would be UB.
//!
//! # Streams
//!
//! stabby ships no stream type, so `impl Stream<Item = T>` returns cross
//! through `unibind-stream`, a small shared crate declaring the
//! `#[stabby::stabby] trait RawStream` protocol (poll-next with a
//! [`StableWaker`], `None` for pending, inner `None` for end). It must be a
//! shared crate: stabby stamps a trait vtable's type report with the module
//! path of its declaration site, so two per-crate declarations would fail
//! the structural check even with identical shapes. Record mirrors dodge
//! the same trap with `#[stabby::stabby(module = ...)]`, which this backend
//! pins to `unibind::<interface>` on both sides.
//!
//! # Boundary contracts the generated code documents
//!
//! Both sides must keep Rust's default global allocator (stabby's default
//! `RustAlloc` frees through whichever allocator the *allocating* side
//! compiled in), and the generated client never exposes library unloading:
//! the `Engine` keeps the cdylib mapped for its whole lifetime because
//! returned values and futures point into it. Exported `async fn`s must
//! produce `Send + Sync` futures (the `DynFuture` bound).

mod client;
mod error;
mod function;
mod loader;
mod module;
mod record;
mod ty;

pub use client::render_client;
pub use module::render;

/// The rendered engine glue for one interface.
#[derive(Debug)]
pub struct RenderedInterface {
    /// A hidden sibling module for the exported module: mirror structs,
    /// stable error carriers, `#[stabby::export]` wrappers, and the IR-hash
    /// handshake symbol.
    pub glue: proc_macro2::TokenStream,
}

/// Options for [`render_client`].
#[derive(Debug)]
pub struct ClientOptions {
    /// The generated crate's `[package] name`.
    pub crate_name: String,
    /// Whether dependencies resolve through `workspace = true` (plus a
    /// repo-shaped `package.nix`) or carry concrete versions for use outside
    /// this workspace.
    pub workspace_deps: bool,
}

/// A generated client crate: every file it consists of.
#[derive(Debug)]
pub struct RenderedCrate {
    /// The crate's files, paths relative to the crate root.
    pub files: Vec<RenderedFile>,
}

/// One generated file.
#[derive(Debug)]
pub struct RenderedFile {
    /// Path relative to the generated crate root, e.g. `src/engine.rs`.
    pub path: String,
    /// The file's full contents, already formatted.
    pub contents: String,
}

/// A rendering failure; the macro positions it at the exported module.
#[derive(Debug)]
pub struct RenderError {
    /// What went wrong and what to do instead.
    pub message: String,
}

impl RenderError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
