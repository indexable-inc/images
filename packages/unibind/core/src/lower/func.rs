//! Lower exported functions, object methods, and constructors.

use syn::spanned::Spanned as _;

use super::ty::{lower_type, Position};
use super::{attrs, marker, ret, Declared, LowerError, Result};
use crate::ir;

/// What a signature lowers as; receivers and return conventions differ.
#[derive(Debug, Clone, Copy)]
pub(super) enum Kind<'a> {
    /// A free `pub fn` in the exported module.
    Free,
    /// An object method; the `&self` receiver was validated by the caller
    /// and is skipped here.
    Method,
    /// An object constructor: sync, receiver-less, returning the object.
    Constructor {
        /// The object type the constructor must return.
        object: &'a str,
    },
}

impl Kind<'_> {
    const fn context(self) -> &'static str {
        match self {
            Self::Free => "a function",
            Self::Method => "a method",
            Self::Constructor { .. } => "a constructor",
        }
    }
}

pub(super) fn lower_fn(func: &syn::ItemFn, declared: &Declared) -> Result<ir::Function> {
    lower_callable(&func.attrs, &func.sig, declared, Kind::Free)
}

pub(super) fn lower_callable(
    attributes: &[syn::Attribute],
    signature: &syn::Signature,
    declared: &Declared,
    kind: Kind<'_>,
) -> Result<ir::Function> {
    if let Some(unsafety) = signature.unsafety {
        return Err(LowerError::new(
            unsafety.span(),
            "unsafe functions do not cross the binding boundary",
        ));
    }
    if !signature.generics.params.is_empty() || signature.generics.where_clause.is_some() {
        return Err(LowerError::new(
            signature.generics.span(),
            "generic functions cannot cross the binding boundary; export a \
             monomorphic wrapper",
        ));
    }
    if let Some(variadic) = &signature.variadic {
        return Err(LowerError::new(
            variadic.span(),
            "variadic functions do not cross the binding boundary",
        ));
    }
    let asyncness = match signature.asyncness {
        Some(token) => {
            if matches!(kind, Kind::Constructor { .. }) {
                return Err(LowerError::new(
                    token.span(),
                    "Python constructors are synchronous; expose an async \
                     factory function instead",
                ));
            }
            ir::Asyncness::Async
        }
        None => ir::Asyncness::Sync,
    };

    let meta = attrs::UnibindMeta::from_attrs(attributes)?;
    meta.reject_default(kind.context())?;
    meta.reject_py_base(kind.context())?;
    meta.reject_backends(kind.context())?;
    meta.reject_resource(kind.context())?;
    match kind {
        // A `constructor` flag routed the signature here already, so only
        // the other kinds can carry it by mistake.
        Kind::Free | Kind::Method => meta.reject_constructor(kind.context())?,
        Kind::Constructor { .. } => meta.reject_blocking(kind.context())?,
    }
    let blocking = meta.blocking;
    if blocking && matches!(asyncness, ir::Asyncness::Async) {
        return Err(LowerError::new(
            meta.span.unwrap_or_else(proc_macro2::Span::call_site),
            "async bodies already run off the GIL; `blocking` applies to \
             sync exports",
        ));
    }

    let mut args = Vec::new();
    for input in &signature.inputs {
        let arg = match input {
            syn::FnArg::Receiver(receiver) => {
                if matches!(kind, Kind::Method) {
                    continue;
                }
                return Err(LowerError::new(
                    receiver.span(),
                    "a free function takes no receiver; methods live in an \
                     impl block for a #[unibind::object] type",
                ));
            }
            syn::FnArg::Typed(arg) => arg,
        };
        let lowered = lower_arg(arg, declared)?;
        if matches!(asyncness, ir::Asyncness::Async) && borrows(&lowered.ty, true) {
            return Err(LowerError::new(
                arg.span(),
                "async exports take owned arguments (String, PathBuf, \
                 Vec<u8>); borrowed data cannot outlive the call into the \
                 Python event loop",
            ));
        }
        if blocking && borrows(&lowered.ty, false) {
            return Err(LowerError::new(
                arg.span(),
                "a blocking export releases the GIL, so it takes owned \
                 String/PathBuf arguments; &[u8] stays zero-copy through the \
                 buffer protocol",
            ));
        }
        args.push(lowered);
    }
    check_default_order(signature, &args)?;

    let returned = match kind {
        Kind::Constructor { object } => ret::lower_ctor_return(&signature.output, object, declared)?,
        Kind::Free | Kind::Method => ret::lower_return(&signature.output, declared)?,
    };
    Ok(ir::Function {
        name: signature.ident.to_string(),
        names: meta.names(),
        docs: marker::doc_lines(attributes),
        asyncness,
        blocking,
        args,
        ret: returned.ty,
        throws: returned.throws,
    })
}

/// Whether `ty` borrows caller data (directly or under `Option`, the only
/// places phase 0 allows borrows); `include_bytes` is off for blocking
/// exports, whose `&[u8]` stays a zero-copy buffer-protocol view.
fn borrows(ty: &ir::Type, include_bytes: bool) -> bool {
    match ty {
        ir::Type::String { owned } | ir::Type::Path { owned } => !owned,
        ir::Type::Bytes { owned } => include_bytes && !owned,
        ir::Type::Option(inner) => borrows(inner, include_bytes),
        _ => false,
    }
}

fn lower_arg(arg: &syn::PatType, declared: &Declared) -> Result<ir::Arg> {
    let syn::Pat::Ident(pattern) = &*arg.pat else {
        return Err(LowerError::new(
            arg.pat.span(),
            "exported function arguments need plain identifier names",
        ));
    };
    let meta = attrs::UnibindMeta::from_attrs(&arg.attrs)?;
    meta.reject_py_base("an argument")?;
    meta.reject_backends("an argument")?;
    meta.reject_resource("an argument")?;
    meta.reject_constructor("an argument")?;
    meta.reject_blocking("an argument")?;
    Ok(ir::Arg {
        name: pattern.ident.to_string(),
        names: meta.names(),
        ty: lower_type(&arg.ty, declared, Position::Arg)?,
        default: meta.default,
    })
}

/// Python only accepts defaulted parameters after other defaulted ones, so
/// enforce the same shape here: once an argument has a default (explicit, or
/// the implicit `None` of an `Option`), every later argument needs one.
fn check_default_order(signature: &syn::Signature, args: &[ir::Arg]) -> Result<()> {
    let mut defaults_started = false;
    for arg in args {
        let has_default = arg.default.is_some() || matches!(arg.ty, ir::Type::Option(_));
        if defaults_started && !has_default {
            return Err(LowerError::new(
                signature.span(),
                format!(
                    "argument `{}` needs a default: it follows a defaulted argument",
                    arg.name
                ),
            ));
        }
        defaults_started = defaults_started || has_default;
    }
    Ok(())
}
