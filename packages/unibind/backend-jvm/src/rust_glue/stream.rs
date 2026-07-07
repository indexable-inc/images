//! Render the companion exports of a stream-returning export:
//! `__stream_next`, `__stream_free`, and `__item_free`.
//!
//! The producing export's envelope carries the stream as an opaque handle;
//! these companions pull items one at a time (pull-based backpressure) and
//! release the handle and each delivered item envelope.

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::model::Model;
use crate::rust_glue::{encode, rusty, types, Export};
use crate::{names, RenderError};

/// The three companion exports for one stream-returning export.
pub(crate) fn companions(
    export: &Export<'_>,
    interface: &ir::Interface,
    model: &Model<'_>,
    user: &Ident,
    item: &ir::Type,
) -> Result<TokenStream, RenderError> {
    let base = export.symbol(interface);
    let next_ident = format_ident!("{}", names::stream_next_suffix(&base));
    let stream_free_ident = format_ident!("{}", names::stream_free_suffix(&base));
    let item_free_ident = format_ident!("{}", names::item_free_suffix(&base));
    let item_envelope = types::item_envelope_ident(export.owner, &export.function.name);
    let item_ty = rusty::rust_type(item, user);
    // The item envelope's payload is `COption<item>`, so the existing
    // option encoder covers both the item and the end-of-stream case.
    let option_encode = encode::expr(
        model,
        &ir::Type::Option(Box::new(item.clone())),
        &quote!(item),
    )?;

    Ok(quote! {
        /// Pull one item from a stream handle produced by the paired
        /// export. `cb` fires exactly once per call with a boxed item
        /// envelope: `code` 0 with a present value is an item, 0 with an
        /// absent value is end-of-stream, -1 is a producer panic. Never
        /// issue a second pull before the previous callback fired.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #next_ident(
            handle: *mut ::core::ffi::c_void,
            cb: unsafe extern "C" fn(*mut ::core::ffi::c_void, *mut #item_envelope),
            user_data: *mut ::core::ffi::c_void,
        ) {
            let user_data = SendPtr(user_data);
            unsafe {
                ::unibind_runtime::jvm::stream_next::<#item_ty>(handle, move |outcome| {
                    let envelope = match outcome {
                        ::unibind_runtime::jvm::NextOutcome::Item(item) => #item_envelope {
                            code: 0,
                            err_msg: null_string(),
                            value: #option_encode,
                        },
                        ::unibind_runtime::jvm::NextOutcome::Panicked(text) => #item_envelope {
                            code: -1,
                            err_msg: string_value(text),
                            value: unsafe { ::core::mem::zeroed() },
                        },
                    };
                    unsafe {
                        cb(user_data.0, ::std::boxed::Box::into_raw(::std::boxed::Box::new(envelope)))
                    }
                })
            }
        }

        /// Release a stream handle produced by the paired export, exactly
        /// once. Safe while a pull is in flight: the pull holds its own
        /// reference.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #stream_free_ident(handle: *mut ::core::ffi::c_void) {
            unsafe { ::unibind_runtime::jvm::stream_free::<#item_ty>(handle) }
        }

        /// Reclaim an item envelope delivered to a `__stream_next`
        /// callback. Null is a no-op; anything else must come from that
        /// callback, exactly once.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn #item_free_ident(envelope: *mut #item_envelope) {
            if envelope.is_null() {
                return;
            }
            drop(unsafe { ::std::boxed::Box::from_raw(envelope) });
        }
    })
}
