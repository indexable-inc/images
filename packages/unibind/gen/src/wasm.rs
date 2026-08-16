//! Emit the browser host files for one interface: the `wasm-bindgen` half of
//! the JavaScript family.
//!
//! The files and everything in them are [`crate::js`]'s; this module is the
//! `wasm-bindgen` flavor of it. The wrapper it asks for is an ES module over
//! the `wasm-bindgen --target web` output at one module specifier: it imports
//! each compiled export under an alias, re-exports the module's own
//! initializer (nothing runs before it is awaited), declares bytes as the
//! `Uint8Array` and number arrays the wasm boundary actually carries, and
//! watches open resources with a `FinalizationRegistry` -- the leak warning
//! `unibind-backend-wasm` deliberately leaves to JavaScript, since wasm has no
//! `Drop`-at-collection story for the warning to hang on.

use unibind_core::ir::Interface;

use crate::host::{EmitError, HostEmitter, HostFile};
use crate::js::{self, Flavor};

/// The wasm emitter; writes `index.d.ts`, `schemas.ts`, and `index.js` at the
/// output root.
pub struct WasmEmitter {
    /// Module specifier of the `wasm-bindgen --target web` JavaScript output
    /// (`./wasm/ix_sdk.js`): the generated `index.js` imports every compiled
    /// export and the initializer from it, so packaging must place that
    /// module -- and the `.wasm` it loads -- where the specifier resolves.
    pub module: String,
}

impl WasmEmitter {
    fn flavor(&self) -> Flavor {
        Flavor::Browser {
            module: self.module.clone(),
        }
    }
}

impl HostEmitter for WasmEmitter {
    fn target(&self) -> &'static str {
        self.flavor().target()
    }

    fn emit(&self, interface: &Interface) -> Result<Vec<HostFile>, EmitError> {
        js::emit(interface, &self.flavor())
    }
}
