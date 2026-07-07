//! Render the opaque box handles for containers the bridge cannot spell.
//!
//! Each box is a newtype over the real Rust value with index accessors; the
//! Swift overlay drains or fills it element by element. Returned maps are
//! sorted by key so iteration order is deterministic on the Swift side.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::names;
use crate::repr::{self, BoxShape};

/// One rendered box: its declarations inside the bridge's `extern "Rust"`
/// block, and its backing items in the glue module.
pub struct RenderedBox {
    pub bridge_decls: TokenStream,
    pub items: TokenStream,
}

/// The constructor function name for a shape (`__unibind_new_vec_of_string`).
pub fn ctor_ident(shape: &BoxShape, suffix: &str) -> Ident {
    let snake = names::to_snake(&shape.mangle());
    Ident::new(&format!("__unibind_new_{snake}{suffix}"), Span::call_site())
}

pub fn render_box(shape: &BoxShape, user: &Ident) -> RenderedBox {
    match shape {
        BoxShape::Vec(inner) => render_vec(shape, inner, user),
        BoxShape::Option(inner) => render_option(shape, inner, user),
        BoxShape::Map { key, value } => render_map(shape, key, value, user),
        BoxShape::Value(inner) => render_value(shape, inner, user),
        BoxShape::Record(_) => unreachable!("records render as handles in record.rs"),
    }
}

/// The single-value carrier for throwing returns ([`BoxShape::Value`]): no
/// bridge constructor (only Rust fills it), one `value()` accessor Swift
/// drains it with.
fn render_value(shape: &BoxShape, inner: &ir::Type, user: &Ident) -> RenderedBox {
    let ident = shape.ident();
    let payload_bridge = repr::bridge_type(inner);
    let payload_rust = repr::rust_type(inner, user);
    let cloned = quote!(self.0.clone());
    let read = repr::to_repr(inner, &cloned);

    let bridge_decls = quote! {
        type #ident;
        fn value(self: &#ident) -> #payload_bridge;
    };
    let items = quote! {
        pub struct #ident(#payload_rust);
        impl #ident {
            fn from_value(value: #payload_rust) -> Self {
                Self(value)
            }
            fn value(&self) -> #payload_bridge {
                #read
            }
        }
    };
    RenderedBox {
        bridge_decls,
        items,
    }
}

fn render_vec(shape: &BoxShape, inner: &ir::Type, user: &Ident) -> RenderedBox {
    let ident = shape.ident();
    let ctor = ctor_ident(shape, "");
    let element_bridge = repr::bridge_type(inner);
    let element_rust = repr::rust_type(inner, user);
    let stored = quote!(value);
    let pushed = repr::from_repr(inner, &stored);
    let cloned = quote!(self.0[index].clone());
    let got = repr::to_repr(inner, &cloned);

    let bridge_decls = quote! {
        type #ident;
        fn #ctor() -> #ident;
        fn push(self: &mut #ident, value: #element_bridge);
        fn len(self: &#ident) -> usize;
        fn get(self: &#ident, index: usize) -> #element_bridge;
    };
    let items = quote! {
        pub struct #ident(::std::vec::Vec<#element_rust>);
        impl #ident {
            fn from_value(value: ::std::vec::Vec<#element_rust>) -> Self {
                Self(value)
            }
            fn into_value(self) -> ::std::vec::Vec<#element_rust> {
                self.0
            }
            fn push(&mut self, value: #element_bridge) {
                self.0.push(#pushed);
            }
            fn len(&self) -> usize {
                self.0.len()
            }
            fn get(&self, index: usize) -> #element_bridge {
                #got
            }
        }
        fn #ctor() -> #ident {
            #ident(::std::vec::Vec::new())
        }
    };
    RenderedBox {
        bridge_decls,
        items,
    }
}

fn render_option(shape: &BoxShape, inner: &ir::Type, user: &Ident) -> RenderedBox {
    let ident = shape.ident();
    let some_ctor = ctor_ident(shape, "_some");
    let none_ctor = ctor_ident(shape, "_none");
    let payload_bridge = repr::bridge_type(inner);
    let payload_rust = repr::rust_type(inner, user);
    let stored = quote!(value);
    let filled = repr::from_repr(inner, &stored);
    let cloned = quote!(self.0.clone().expect("unibind option read while none"));
    let read = repr::to_repr(inner, &cloned);

    let bridge_decls = quote! {
        type #ident;
        fn #some_ctor(value: #payload_bridge) -> #ident;
        fn #none_ctor() -> #ident;
        fn is_some(self: &#ident) -> bool;
        fn value(self: &#ident) -> #payload_bridge;
    };
    let items = quote! {
        pub struct #ident(::std::option::Option<#payload_rust>);
        impl #ident {
            fn from_value(value: ::std::option::Option<#payload_rust>) -> Self {
                Self(value)
            }
            fn into_value(self) -> ::std::option::Option<#payload_rust> {
                self.0
            }
            fn is_some(&self) -> bool {
                self.0.is_some()
            }
            /// The payload; the overlay only calls this behind `is_some`.
            fn value(&self) -> #payload_bridge {
                #read
            }
        }
        fn #some_ctor(value: #payload_bridge) -> #ident {
            #ident(::std::option::Option::Some(#filled))
        }
        fn #none_ctor() -> #ident {
            #ident(::std::option::Option::None)
        }
    };
    RenderedBox {
        bridge_decls,
        items,
    }
}

fn render_map(shape: &BoxShape, key: &ir::Type, value: &ir::Type, user: &Ident) -> RenderedBox {
    let ident = shape.ident();
    let ctor = ctor_ident(shape, "");
    let key_bridge = repr::bridge_type(key);
    let value_bridge = repr::bridge_type(value);
    let key_rust = repr::rust_type(key, user);
    let value_rust = repr::rust_type(value, user);
    let key_expr = quote!(key);
    let value_expr = quote!(value);
    let inserted_key = repr::from_repr(key, &key_expr);
    let inserted_value = repr::from_repr(value, &value_expr);
    let key_cloned = quote!(self.0[index].0.clone());
    let key_read = repr::to_repr(key, &key_cloned);
    let value_cloned = quote!(self.0[index].1.clone());
    let value_read = repr::to_repr(value, &value_cloned);

    let bridge_decls = quote! {
        type #ident;
        fn #ctor() -> #ident;
        fn insert(self: &mut #ident, key: #key_bridge, value: #value_bridge);
        fn len(self: &#ident) -> usize;
        fn key_at(self: &#ident, index: usize) -> #key_bridge;
        fn value_at(self: &#ident, index: usize) -> #value_bridge;
    };
    let items = quote! {
        pub struct #ident(::std::vec::Vec<(#key_rust, #value_rust)>);
        impl #ident {
            /// Entries sorted by key, so Swift-side iteration is
            /// deterministic (`HashMap` order is randomized per process).
            fn from_value(value: ::std::collections::HashMap<#key_rust, #value_rust>) -> Self {
                let mut entries: ::std::vec::Vec<(#key_rust, #value_rust)> =
                    value.into_iter().collect();
                entries.sort_by(|left, right| left.0.cmp(&right.0));
                Self(entries)
            }
            fn into_value(self) -> ::std::collections::HashMap<#key_rust, #value_rust> {
                self.0.into_iter().collect()
            }
            fn insert(&mut self, key: #key_bridge, value: #value_bridge) {
                self.0.push((#inserted_key, #inserted_value));
            }
            fn len(&self) -> usize {
                self.0.len()
            }
            fn key_at(&self, index: usize) -> #key_bridge {
                #key_read
            }
            fn value_at(&self, index: usize) -> #value_bridge {
                #value_read
            }
        }
        fn #ctor() -> #ident {
            #ident(::std::vec::Vec::new())
        }
    };
    RenderedBox {
        bridge_decls,
        items,
    }
}
