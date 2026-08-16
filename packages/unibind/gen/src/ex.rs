//! Emit the Elixir host files for one interface.
//!
//! Two files land under `lib/`: `lib/<app>/native.ex` with the NIF stubs
//! behind `@on_load`, and `lib/<app>.ex` with the typespec'd public
//! wrapper. A third, `mix.exs`, makes the output a mix package rather than
//! loose modules; its app name and namespace come from the same interface,
//! so the project and the modules in it cannot name different apps. The
//! rendering itself lives in `unibind-backend-ex` (next to the glue
//! renderer, so NIF names and arities cannot drift apart); this emitter only
//! decides where the files land.

use unibind_core::docs;
use unibind_core::ir::Interface;

use crate::host::{EmitError, HostEmitter, HostFile};

/// The Elixir emitter; writes into `lib/` under the output root.
pub struct ExEmitter {
    /// File name of the built NIF library (`libmylib.so`); the generated
    /// load call strips the extension, as `:erlang.load_nif/2` expects.
    pub nif_soname: String,
}

impl HostEmitter for ExEmitter {
    fn target(&self) -> &'static str {
        "ex"
    }

    fn emit(&self, interface: &Interface) -> Result<Vec<HostFile>, EmitError> {
        // Intra-doc links resolve into this language's spelling before
        // anything renders a doc comment; see `unibind_core::docs`.
        let interface = &docs::resolve(interface, docs::Language::Ex)
            .map_err(|error| EmitError { message: error.to_string() })?;
        let modules =
            unibind_backend_ex::host_modules(interface, &self.nif_soname).map_err(|error| {
                EmitError {
                    message: error.message,
                }
            })?;
        let app = modules.app;
        Ok(vec![
            HostFile {
                path: format!("lib/{app}/native.ex"),
                contents: modules.native,
            },
            HostFile {
                path: format!("lib/{app}.ex"),
                contents: modules.wrapper,
            },
            HostFile {
                path: "mix.exs".to_owned(),
                contents: modules.mix_project,
            },
        ])
    }
}
