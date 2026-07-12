//! Generate the Elixir host modules from the interface.

mod calls;
mod native;
mod typespec;
mod wrapper;

use unibind_core::ir;

use crate::{names, RenderError};

/// The rendered Elixir side of the boundary. `unibind-gen`'s `ExEmitter`
/// decides where the modules land (`lib/<app>/native.ex`, `lib/<app>.ex`).
pub struct HostModules {
    /// The `snake_case` namespace: the OTP app and the `.ex` file names.
    pub app: String,
    /// The `<Ns>.Native` module: NIF stubs behind `@on_load`.
    pub native: String,
    /// The `<Ns>` module: typespec'd public wrapper functions.
    pub wrapper: String,
}

/// Render the Elixir side of the boundary.
///
/// `nif_soname` is the file name of the built NIF library (`libmylib.so`);
/// the generated load call strips the extension, as `:erlang.load_nif/2`
/// expects.
///
/// # Errors
///
/// Mirrors [`crate::render`]'s rejections: the two sides come from the same
/// interface and must agree.
pub fn host_modules(
    interface: &ir::Interface,
    nif_soname: &str,
) -> Result<HostModules, RenderError> {
    // One validator for both sides: whatever the glue renderer rejects, the
    // host modules must not paper over.
    crate::module::render(interface, None)?;
    Ok(HostModules {
        app: names::ns_snake(interface),
        native: native::render(interface, nif_soname),
        wrapper: wrapper::render(interface),
    })
}
