//! Emit the TypeScript host files for one interface.
//!
//! Three files land at the npm package root: `index.d.ts` types every export
//! (`TSDoc` from the IR's doc comments), `schemas.ts` carries a Zod schema
//! per record for consumers that want the same shapes checked at runtime,
//! and the `CommonJS` `index.js` wraps the native addon into the surface
//! users import: decoded `Error` subclasses, async functions forwarding a
//! trailing `AbortSignal`, streams as `AsyncIterable`s, and object classes
//! with the resource close surface (`await using` works). The wrapper pairs
//! with the glue the `ts`-feature macro backend (`unibind-backend-ts`)
//! compiled into the addon: everything dynamic crosses inside `__unibind__:`
//! napi rejection reasons, and `index.js` is where those reasons become real
//! exception types.

mod dts;
mod js;
mod types;
mod zod;

use unibind_core::ir::Interface;

use crate::host::{EmitError, HostEmitter, HostFile};

/// The TypeScript emitter; writes `index.d.ts`, `schemas.ts`, and
/// `index.js` at the output root.
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
        let mut files = vec![HostFile {
            path: "index.d.ts".to_owned(),
            contents: dts::render(interface)?,
        }];
        // Records are the only thing with a Zod schema, so an interface
        // without one would land a file whose sole content is an unused
        // `zod` import -- and a `zod` peer dependency the package does not
        // need. No flag: the schemas come from the same IR as the types, so
        // making them optional would only let the two drift.
        if !interface.records.is_empty() {
            files.push(HostFile {
                path: "schemas.ts".to_owned(),
                contents: zod::render(interface)?,
            });
        }
        files.push(HostFile {
            path: "index.js".to_owned(),
            contents: js::render(interface, &self.addon),
        });
        Ok(files)
    }
}
