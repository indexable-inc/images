//! Emit the TypeScript host files for one interface: the node half of the
//! JavaScript family.
//!
//! The files and everything in them are [`crate::js`]'s; this module is the
//! napi flavor of it. The wrapper it asks for is `CommonJS`, loads
//! `./native/<addon>.node`, normalizes `null` arguments away for napi, and
//! assigns each export individually so Node's cjs-module-lexer sees named
//! exports.

use unibind_core::ir::Interface;

use crate::host::{EmitError, HostEmitter, HostFile};
use crate::js::{self, Flavor};

/// The TypeScript emitter; writes `index.d.ts`, `schemas.ts`, and
/// `index.js` at the output root.
pub struct TsEmitter {
    /// Basename of the native addon: the generated `index.js` loads
    /// `./native/<addon>.node`, so the packaging step must place the
    /// compiled cdylib there.
    pub addon: String,
}

impl TsEmitter {
    fn flavor(&self) -> Flavor {
        Flavor::Node {
            addon: self.addon.clone(),
        }
    }
}

impl HostEmitter for TsEmitter {
    fn target(&self) -> &'static str {
        self.flavor().target()
    }

    fn emit(&self, interface: &Interface) -> Result<Vec<HostFile>, EmitError> {
        js::emit(interface, &self.flavor())
    }
}
