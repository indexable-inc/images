//! Emit the TypeScript host files for one interface.
//!
//! Two files land at the npm package root: `index.d.ts` types every export
//! (`TSDoc` from the IR's doc comments), and the `CommonJS` `index.js`
//! wraps the native addon into the surface users import: decoded `Error`
//! subclasses, async functions forwarding a trailing `AbortSignal`,
//! streams as `AsyncIterable`s, and object classes with the resource close
//! surface (`await using` works). The wrapper pairs with the glue the
//! `ts`-feature macro backend (`unibind-backend-ts`) compiled into the
//! addon: everything dynamic crosses inside `__unibind__:` napi rejection
//! reasons, and `index.js` is where those reasons become real exception
//! types.

mod dts;
mod js;
mod types;

use unibind_core::ir::Interface;

use crate::host::{EmitError, HostEmitter, HostFile};

/// The TypeScript emitter; writes `index.d.ts` and `index.js` at the
/// output root.
pub struct TsEmitter {
    /// Basename of the native addon: the generated `index.js` loads
    /// `./native/<addon>.node`, so the packaging step must place the
    /// compiled cdylib there.
    pub addon: String,
}

impl HostEmitter for TsEmitter {
    fn target(&self) -> &'static str {
        "ts"
    }

    fn emit(&self, interface: &Interface) -> Result<Vec<HostFile>, EmitError> {
        if let Some(data_enum) = interface.enums.first() {
            return Err(EmitError {
                message: format!(
                    "`{}` is a data enum, which the ts backend does not render",
                    data_enum.name
                ),
            });
        }
        Ok(vec![
            HostFile {
                path: "index.d.ts".to_owned(),
                contents: dts::render(interface)?,
            },
            HostFile {
                path: "index.js".to_owned(),
                contents: js::render(interface, &self.addon),
            },
        ])
    }
}
