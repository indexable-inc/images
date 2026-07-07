//! Render the per-object handle release export.
//!
//! Constructors and methods flow through the shared function/async
//! renderers (an object handle is just another export site); the only
//! object-specific export is `<O>__free`, releasing the `Arc` strong count
//! the constructor or a returning function leaked to Java.

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::{names, RenderError};

/// The `<O>__free` export dropping one object handle.
pub(crate) fn free_export(
    object: &ir::Object,
    interface: &ir::Interface,
    user: &Ident,
) -> Result<TokenStream, RenderError> {
    let free_ident = format_ident!(
        "{}",
        names::object_free_symbol(&interface.name, &object.name)
    );
    let object_ident = names::rust_ident(&object.name)?;
    let doc = format!(
        "Release one `{}` handle, exactly once; null is a no-op. In-flight async method \
         tasks own their own strong count, so freeing during a call is safe.",
        object.name
    );
    Ok(quote! {
        #[doc = #doc]
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #free_ident(this: *mut ::core::ffi::c_void) {
            if this.is_null() {
                return;
            }
            drop(unsafe {
                ::std::sync::Arc::from_raw(this.cast_const().cast::<super::#user::#object_ident>())
            });
        }
    })
}
