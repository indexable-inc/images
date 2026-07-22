//! Render a lowered [`unibind_core::ir::Interface`] into C-ABI shims and
//! the Java class that calls them through the FFM API.
//!
//! Unlike the other backends, the jvm backend targets no binding library:
//! every exported function becomes one `extern "C"` symbol with the uniform
//! shape `fn(args: *const u8, len: usize, out: *mut RawBuf)`, and values
//! cross in `unibind-jvm-runtime`'s length-prefixed wire format. The
//! matching Java side is a single generated class (records, exception
//! hierarchy, wire codec, FFM plumbing) from [`host_class`], which
//! `unibind-gen`'s `JvmEmitter` writes to disk. The consuming crate builds
//! a `cdylib` and depends on `unibind-jvm-runtime` directly; the JVM needs
//! `--enable-native-access` for the generated lookups.

mod error;
mod function;
mod host;
mod module;
mod names;
mod record;
mod ty;

pub use host::{HostClass, host_class};
pub use module::render;
