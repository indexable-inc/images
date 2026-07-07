//! Serialize the interface into a link-section constant.
//!
//! The macro plants the JSON-serialized [`Interface`] as a `#[used]` static
//! in a dedicated section of the built artifact, the way `wasm-bindgen`
//! ships its metadata. Later phases (the `unibind-gen` host-file generator,
//! issue #1991) read the interface straight from the compiled library, so
//! generated `.pyi` stubs or `.d.ts` files never need the Rust source.

use proc_macro2::TokenStream;
use quote::quote;

use crate::ir::Interface;
use crate::lower::LowerError;

/// Section name in ELF (and other non-Apple) objects.
pub const LINK_SECTION_ELF: &str = ".unibind_ir";

/// Mach-O `segment,section` pair; section names cap at 16 bytes.
pub const LINK_SECTION_MACH_O: &str = "__DATA,__unibind_ir";

/// The exact bytes the link section carries: one compact `serde_json`
/// rendering of the interface. The Rust client backend hashes these same
/// bytes for its load-time handshake, so the serialization lives here once
/// and the two consumers cannot drift byte-wise.
///
/// # Errors
///
/// Fails only when the interface cannot serialize, which would be a bug in
/// the IR types rather than in the annotated module.
pub fn ir_json(interface: &Interface) -> Result<Vec<u8>, LowerError> {
    serde_json::to_vec(interface).map_err(|error| LowerError {
        span: proc_macro2::Span::call_site(),
        message: format!("serializing the unibind interface failed: {error}"),
    })
}

/// Render the `#[used]` static carrying the serialized `interface`.
///
/// # Errors
///
/// Fails only when the interface cannot serialize, which would be a bug in
/// the IR types rather than in the annotated module.
pub fn ir_static(interface: &Interface) -> Result<TokenStream, LowerError> {
    let json = ir_json(interface)?;
    let len = json.len();
    let bytes = proc_macro2::Literal::byte_string(&json);
    Ok(quote! {
        const _: () = {
            #[cfg_attr(target_vendor = "apple", unsafe(link_section = #LINK_SECTION_MACH_O))]
            #[cfg_attr(not(target_vendor = "apple"), unsafe(link_section = #LINK_SECTION_ELF))]
            #[used]
            static UNIBIND_IR: [u8; #len] = *#bytes;
        };
    })
}
