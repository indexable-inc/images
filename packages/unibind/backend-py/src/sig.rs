//! Shared signature machinery for free functions, methods, and
//! constructors: argument lowering (including buffer-protocol arguments),
//! and the return conversion that routes streams and objects through their
//! glue classes.

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use unibind_core::ir;

use crate::ctx::Ctx;
use crate::{stream, ty, RenderError};

/// A wrapper's argument surface, ready to splice into the generated `fn`.
pub struct Args {
    /// Wrapper parameters (`name: Ty`). A borrowed-bytes argument becomes
    /// `PyBuffer<u8>` so Python passes any buffer-protocol object
    /// zero-copy instead of forcing a `bytes` copy.
    pub params: Vec<TokenStream>,
    /// `#[pyo3(signature = ...)]` entries, defaults included.
    pub signature: Vec<TokenStream>,
    /// Statements before the user call: buffer contiguity checks plus the
    /// borrowed-slice bindings.
    pub prologue: TokenStream,
    /// Argument identifiers forwarded to the user's function.
    pub forwarded: Vec<TokenStream>,
    /// Whether the prologue can fail: a buffer check forces a `PyResult`
    /// return even on functions that do not throw.
    pub fallible: bool,
}

pub fn lower_args(function: &ir::Function, ctx: &Ctx<'_>) -> Result<Args, RenderError> {
    let mut args = Args {
        params: Vec::new(),
        signature: Vec::new(),
        prologue: TokenStream::new(),
        forwarded: Vec::new(),
        fallible: false,
    };
    for arg in &function.args {
        let ident = ty::name_ident(arg.names.py.as_ref().unwrap_or(&arg.name))?;
        if matches!(arg.ty, ir::Type::Bytes { owned: false }) {
            args.params.push(quote!(#ident: ::pyo3::buffer::PyBuffer<u8>));
            args.prologue.extend(buffer_prologue(&ident, arg));
            args.fallible = true;
        } else {
            let ty = ty::rust_type(&arg.ty, ctx.user);
            args.params.push(quote!(#ident: #ty));
        }
        args.signature.push(signature_entry(arg, &ident));
        args.forwarded.push(quote!(#ident));
    }
    Ok(args)
}

/// Turn a `PyBuffer<u8>` parameter into the `&[u8]` the user function
/// expects. The slice shadows the buffer's name AFTER pointer and length
/// are pulled into locals, so the `PyBuffer` itself (and the `Py_buffer`
/// view keeping the exporter's memory alive) stays in scope for the whole
/// call; only its name is taken over by the borrow.
fn buffer_prologue(ident: &Ident, arg: &ir::Arg) -> TokenStream {
    let py_name = arg.names.py.as_ref().unwrap_or(&arg.name);
    let message = format!(
        "argument `{py_name}` must be a C-contiguous buffer \
         (bytes, bytearray, or a contiguous memoryview)"
    );
    // Raw identifiers (`r#type`) keep only the name part in the helpers.
    let raw = ident.to_string();
    let base = raw.trim_start_matches("r#");
    let ptr = format_ident!("__unibind_{base}_ptr");
    let len = format_ident!("__unibind_{base}_len");
    // quote! cannot emit `//` comments, so the safety argument rides on the
    // statement as an allow-with-reason the reader (and a user crate that
    // denies `unsafe_code`) both see.
    quote! {
        if !#ident.is_c_contiguous() {
            return ::pyo3::PyResult::Err(::pyo3::exceptions::PyBufferError::new_err(#message));
        }
        let #ptr = #ident.buf_ptr().cast::<u8>();
        let #len = #ident.item_count();
        #[allow(
            unsafe_code,
            reason = "SAFETY: contiguity was checked above, and the shadowed PyBuffer keeps \
                      its Py_buffer view alive for the whole call, which the buffer protocol \
                      contract says pins the exporter's memory unresized and unfreed"
        )]
        let #ident: &[u8] = unsafe { ::std::slice::from_raw_parts(#ptr, #len) };
    }
}

fn signature_entry(arg: &ir::Arg, ident: &Ident) -> TokenStream {
    if let Some(default) = &arg.default {
        let default = ty::default_tokens(default);
        return quote!(#ident = #default);
    }
    if matches!(arg.ty, ir::Type::Option(_)) {
        return quote!(#ident = None);
    }
    quote!(#ident)
}

/// How a return value crosses: the wrapper's success type and, for streams
/// and objects, the glue class the raw value is wrapped in.
pub struct RetSpec {
    pub ok_ty: TokenStream,
    pub wrap: Option<Ident>,
}

/// `owner` is the object name for methods, `None` for free functions; the
/// per-export stream class is named from it.
pub fn ret_spec(function: &ir::Function, owner: Option<&str>, ctx: &Ctx<'_>) -> RetSpec {
    match &function.ret {
        None => RetSpec {
            ok_ty: quote!(()),
            wrap: None,
        },
        Some(ir::Type::Stream(_)) => {
            let class = stream::class_ident(owner, &function.name);
            RetSpec {
                ok_ty: quote!(#class),
                wrap: Some(class),
            }
        }
        Some(ir::Type::Named(name)) if ctx.is_object(name) => {
            let class = crate::ctx::object_wrapper_ident(name);
            RetSpec {
                ok_ty: quote!(#class),
                wrap: Some(class),
            }
        }
        Some(other) => RetSpec {
            ok_ty: ty::rust_type(other, ctx.user),
            wrap: None,
        },
    }
}

/// Route `value` through the glue class when the return needs wrapping.
pub fn wrap_value(value: TokenStream, wrap: Option<&Ident>) -> TokenStream {
    match wrap {
        Some(class) => quote!(#class::__unibind_wrap(#value)),
        None => value,
    }
}

/// A wrapper's return type and body, which vary together on `throws` and
/// on whether a buffer prologue can fail.
pub struct BodyAndRet {
    pub ret: TokenStream,
    pub body: TokenStream,
}

/// Finish a sync body: `raw` is the expression producing the user's value
/// (the plain call, or the `detach` closure around it).
pub fn finish_sync(raw: &TokenStream, throws: bool, fallible: bool, ret: &RetSpec) -> BodyAndRet {
    let ok_ty = &ret.ok_ty;
    if throws {
        let body = ret.wrap.as_ref().map_or_else(
            || quote!(#raw.map_err(::pyo3::PyErr::from)),
            |class| quote!(#raw.map(#class::__unibind_wrap).map_err(::pyo3::PyErr::from)),
        );
        return BodyAndRet {
            ret: quote!(::pyo3::PyResult<#ok_ty>),
            body,
        };
    }
    let wrapped = wrap_value(raw.clone(), ret.wrap.as_ref());
    if fallible {
        // The buffer contiguity check can fail, so the wrapper returns
        // PyResult even though the user function cannot.
        return BodyAndRet {
            ret: quote!(::pyo3::PyResult<#ok_ty>),
            body: quote!(::pyo3::PyResult::Ok(#wrapped)),
        };
    }
    BodyAndRet {
        ret: quote!(#ok_ty),
        body: wrapped,
    }
}
