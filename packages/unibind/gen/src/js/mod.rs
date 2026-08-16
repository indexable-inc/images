//! Emit the JavaScript-family host files for one interface.
//!
//! Three files land at the package root: `index.d.ts` types every export
//! (`TSDoc` from the IR's doc comments), `schemas.ts` carries a Zod schema per
//! record and per enumeration for consumers that want the same shapes checked
//! at runtime, and `index.js` wraps the compiled artifact into the surface
//! users import: decoded `Error` subclasses, async functions forwarding a
//! trailing `AbortSignal`, streams as `AsyncIterable`s, and object classes
//! with the resource close surface (`await using` works).
//!
//! Two targets share all of it. `unibind-gen ts` renders the napi addon's
//! wrapper for node, `unibind-gen wasm` the `wasm-bindgen` module's wrapper
//! for a browser, and the wrapper pairs with the glue its macro backend
//! compiled in: everything dynamic crosses inside `__unibind__:` failure
//! messages, on one wire vocabulary both backends spell, and `index.js` is
//! where those messages become real exception types. The places the two
//! genuinely differ are the [`Flavor`] value and nothing else.

mod dts;
mod flavor;
mod types;
mod wrapper;
mod zod;

use unibind_core::docs;
use unibind_core::ir::Interface;

pub use flavor::Flavor;

use crate::host::{EmitError, HostFile};

/// Render the three files for one interface in `flavor`.
///
/// # Errors
///
/// Fails for interface surface the flavor's backend cannot express (see
/// [`types::ts_type`]) and for doc comments whose intra-doc links do not
/// resolve.
pub fn emit(interface: &Interface, flavor: &Flavor) -> Result<Vec<HostFile>, EmitError> {
    // Doc comments are written against the Rust surface, so their intra-doc
    // links are resolved into TSDoc `{@link ...}` references here, once,
    // before any of the three files renders one. Both flavors publish
    // TypeScript declarations, so both resolve the same way.
    let interface = &docs::resolve(interface, docs::Language::Ts)
        .map_err(|error| EmitError { message: error.to_string() })?;
    let mut files = vec![HostFile {
        path: "index.d.ts".to_owned(),
        contents: dts::render(interface, flavor)?,
    }];
    // Records and enumerations are the only things with a Zod schema, so an
    // interface without either would land a file whose sole content is an
    // unused `zod` import -- and a `zod` peer dependency the package does not
    // need. No flag: the schemas come from the same IR as the types, so making
    // them optional would only let the two drift.
    if !interface.records.is_empty() || !interface.enums.is_empty() {
        files.push(HostFile {
            path: "schemas.ts".to_owned(),
            contents: zod::render(interface, flavor)?,
        });
    }
    files.push(HostFile {
        path: "index.js".to_owned(),
        contents: wrapper::render(interface, flavor),
    });
    Ok(files)
}
