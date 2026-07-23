//! The `builtins.wasm` face of [`ix2nix`]: a `wasm32-unknown-unknown` cdylib
//! exporting `convert(ValueId) -> ValueId` against the patched Nix
//! evaluator's non-WASI host interface (`src/libexpr/primops/wasm.cc`).
//!
//! The evaluator instantiates the module, calls the exported
//! `nix_wasm_init_v1()`, then calls the configured function with the argument
//! value's id. Host functions are imported from module `env`; this plugin
//! needs only three of them:
//!
//! - `copy_string(value, ptr, max_len) -> len` forces a Nix string value and
//!   copies it into linear memory. The host writes only when the string fits
//!   (`len <= max_len`) and always returns the true length, so one call with
//!   `max_len = 0` sizes the buffer and a second call fills it.
//! - `make_string(ptr, len) -> value` allocates a Nix string value from
//!   linear memory.
//! - `panic(ptr, len)` traps with the message. The trap propagates out of the
//!   Wasm call as a Nix eval error (`Wasm panic: <message>`) carrying a
//!   positioned "while executing the Wasm function from '...'" trace, which
//!   is how a conversion failure surfaces its rendered [`ix2nix::Error`]
//!   diagnostic verbatim.
//!
//! All buffers live in this module's exported linear memory (the host only
//! reads and writes it), so plain `String`/`Vec` allocations are the whole
//! transfer protocol.

#[cfg(target_arch = "wasm32")]
mod host {
    // `wasm_import_module` marks these as Wasm imports from module `env`
    // (where wasm.cc registers its host functions) instead of symbols the
    // linker must resolve.
    #[link(wasm_import_module = "env")]
    unsafe extern "C" {
        pub fn copy_string(value: u32, ptr: u32, max_len: u32) -> u32;
        pub fn make_string(ptr: u32, len: u32) -> u32;
        pub fn panic(ptr: u32, len: u32) -> !;
    }
}

#[cfg(target_arch = "wasm32")]
mod plugin {
    use crate::host;

    /// A byte range of linear memory in the `(ptr, len)` pair the host
    /// interface speaks. Both fit `u32` on wasm32, where `usize` is 32 bits.
    struct HostSlice {
        ptr: u32,
        len: u32,
    }

    fn host_slice(bytes: &[u8]) -> HostSlice {
        HostSlice {
            ptr: u32::try_from(bytes.as_ptr().addr()).expect("wasm32 addresses fit u32"),
            len: u32::try_from(bytes.len()).expect("wasm32 lengths fit u32"),
        }
    }

    /// Traps with `message`; the evaluator turns the trap into a positioned
    /// Nix eval error at the `builtins.wasm` call site.
    pub fn trap(message: &str) -> ! {
        let message = host_slice(message.as_bytes());
        unsafe { host::panic(message.ptr, message.len) }
    }

    /// Reads the Nix string value `value_id` out of the evaluator.
    fn nix_string(value_id: u32) -> String {
        // SAFETY: `max_len = 0` means the host only reports the length; the
        // pointer is never dereferenced.
        let len = unsafe { host::copy_string(value_id, 0, 0) };

        let mut buffer = vec![0_u8; usize::try_from(len).expect("u32 fits usize")];
        let ptr = u32::try_from(buffer.as_mut_ptr().addr()).expect("wasm32 addresses fit u32");
        // SAFETY: `buffer` stays alive across the call and is exactly `len`
        // bytes, so the host's write is in bounds; the mutable borrow above
        // keeps Rust references off the bytes the host writes.
        let written = unsafe { host::copy_string(value_id, ptr, len) };
        if written != len {
            trap("host changed a string's length between copy_string calls");
        }

        match String::from_utf8(buffer) {
            Ok(source) => source,
            Err(_) => trap("the .ix source is not valid UTF-8"),
        }
    }

    /// Allocates a Nix string value holding `value`.
    fn make_nix_string(value: &str) -> u32 {
        let span = host_slice(value.as_bytes());
        // SAFETY: `value` stays alive across the call; the host copies the
        // bytes out before returning.
        unsafe { host::make_string(span.ptr, span.len) }
    }

    /// `convert` entry: `.ix` source string in, Nix source string out; a
    /// conversion error traps with the rendered diagnostic.
    pub fn convert(source_id: u32) -> u32 {
        let source = nix_string(source_id);
        match ix2nix::convert(&source) {
            Ok(nix_source) => make_nix_string(&nix_source),
            Err(error) => trap(&error.to_string()),
        }
    }
}

/// Handshake export the evaluator calls before the entry function. Routes
/// Rust panics (which otherwise abort as an opaque `unreachable` trap) through
/// the host's `panic` import so their message survives into the eval error.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn nix_wasm_init_v1() {
    std::panic::set_hook(Box::new(|info| plugin::trap(&info.to_string())));
}

/// The `builtins.wasm { function = "convert"; }` entry point.
#[cfg(target_arch = "wasm32")]
#[unsafe(no_mangle)]
pub extern "C" fn convert(source_id: u32) -> u32 {
    plugin::convert(source_id)
}
