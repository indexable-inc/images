//! Register objects as BEAM resources and render their members.

use heck::ToSnakeCase as _;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::function::{borrowed_params, nif_attr, Params};
use crate::{error, names, ty, RenderError};

/// One `resource_impl` registration plus a NIF per constructor and method.
/// The user's struct itself gains nothing: `ResourceArc` wraps it as an
/// opaque reference, and the user's `Drop` runs when the BEAM collects it.
pub fn render_object(object: &ir::Object, user: &Ident) -> Result<TokenStream, RenderError> {
    let obj_ident = Ident::new(&object.name, Span::call_site());
    let mut items = vec![quote! {
        #[::rustler::resource_impl]
        impl ::rustler::Resource for super::#user::#obj_ident {}
    }];
    if let Some(constructor) = &object.constructor {
        items.push(render_constructor(object, constructor, user, &obj_ident)?);
    }
    for method in &object.methods {
        items.push(render_method(object, method, user, &obj_ident)?);
    }
    Ok(quote!(#(#items)*))
}

/// The glue wrapper identifier for an object member, unique per object.
fn member_ident(object: &ir::Object, function: &ir::Function) -> Ident {
    Ident::new(
        &format!("{}_{}", object.name.to_snake_case(), function.name),
        Span::call_site(),
    )
}

/// Async and stream members are stage 2 of the elixir backend; the sync
/// surface ships first.
fn reject_suspending(object: &ir::Object, function: &ir::Function) -> Result<(), RenderError> {
    if matches!(function.asyncness, ir::Asyncness::Async) {
        return Err(RenderError::new(format!(
            "`{}::{}` is async; async object members are not part of the \
             elixir backend yet, expose a free function",
            object.name, function.name
        )));
    }
    if matches!(function.ret, Some(ir::Type::Stream(_))) {
        return Err(RenderError::new(format!(
            "`{}::{}` returns a stream; stream object members are not part \
             of the elixir backend yet, expose a free function",
            object.name, function.name
        )));
    }
    Ok(())
}

fn render_constructor(
    object: &ir::Object,
    constructor: &ir::Function,
    user: &Ident,
    obj_ident: &Ident,
) -> Result<TokenStream, RenderError> {
    reject_suspending(object, constructor)?;
    let wrapper = member_ident(object, constructor);
    let attr = nif_attr(
        constructor,
        &names::member_nif_name(object, constructor),
        &wrapper,
    );
    let rust_name = Ident::new(&constructor.name, Span::call_site());
    let Params { params, forwarded } = borrowed_params(constructor, user)?;
    let call = quote!(super::#user::#obj_ident::#rust_name(#(#forwarded),*));
    let handle = quote!(::rustler::ResourceArc<super::#user::#obj_ident>);
    if let Some(throws) = &constructor.throws {
        let term = error::term_ident(throws);
        return Ok(quote! {
            #attr
            fn #wrapper(#(#params),*) -> ::std::result::Result<#handle, #term> {
                #call.map(::rustler::ResourceArc::new).map_err(#term::from)
            }
        });
    }
    Ok(quote! {
        #attr
        fn #wrapper(#(#params),*) -> #handle {
            ::rustler::ResourceArc::new(#call)
        }
    })
}

fn render_method(
    object: &ir::Object,
    method: &ir::Function,
    user: &Ident,
    obj_ident: &Ident,
) -> Result<TokenStream, RenderError> {
    reject_suspending(object, method)?;
    let wrapper = member_ident(object, method);
    let attr = nif_attr(method, &names::member_nif_name(object, method), &wrapper);
    let rust_name = Ident::new(&method.name, Span::call_site());
    let Params { params, forwarded } = borrowed_params(method, user)?;
    let ok_ty = method.ret.as_ref().map_or_else(
        || Ok(quote!(())),
        |ret| {
            ty::rust_type(ret, user).map_err(|error| {
                RenderError::new(format!(
                    "return of `{}::{}`: {}",
                    object.name, method.name, error.message
                ))
            })
        },
    )?;
    let call = quote!(handle.#rust_name(#(#forwarded),*));
    let handle_ty = quote!(::rustler::ResourceArc<super::#user::#obj_ident>);
    if let Some(throws) = &method.throws {
        let term = error::term_ident(throws);
        return Ok(quote! {
            #attr
            fn #wrapper(
                handle: #handle_ty,
                #(#params),*
            ) -> ::std::result::Result<#ok_ty, #term> {
                #call.map_err(#term::from)
            }
        });
    }
    Ok(quote! {
        #attr
        fn #wrapper(handle: #handle_ty, #(#params),*) -> #ok_ty {
            #call
        }
    })
}
