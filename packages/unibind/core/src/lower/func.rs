//! Lower exported functions, object methods, and constructors.

use syn::spanned::Spanned as _;

use super::ty::{Position, lower_type};
use super::{Declared, LowerError, Result, attrs, marker, ret};
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
    /// A named function on the object rather than on an instance: no
    /// receiver, may be async, and there may be several. Two shapes share
    /// this kind because they differ only in what they return. One
    /// constructs the object, which is what a constructor cannot do when it
    /// has to await first, since Python's `__new__` and napi's
    /// `constructor` are both synchronous. The other answers something
    /// else about the type, `Machine.list()` being the case that forced it.
    /// Which one a given function is falls out of its return type, so the
    /// author never says it twice.
    Associated {
        /// The object it belongs to, which resolves a `Self` return and
        /// scopes per-export stream classes.
        object: &'a str,
    },
}

impl Kind<'_> {
    const fn context(self) -> &'static str {
        match self {
            Self::Free => "a function",
            Self::Method => "a method",
            Self::Constructor { .. } => "a constructor",
            Self::Associated { .. } => "an associated function",
        }
    }
}

pub(super) fn lower_fn(func: &syn::ItemFn, declared: &Declared) -> Result<ir::Function> {
    lower_callable(Callable {
        attributes: &func.attrs,
        signature: &func.sig,
        declared,
        kind: Kind::Free,
    })
}

/// Reject signature shapes that never cross the binding boundary (unsafe,
/// generic, variadic); split out of [`lower_callable`] to keep it within
/// clippy's function-length budget.
fn reject_unsupported(signature: &syn::Signature) -> Result<()> {
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
    Ok(())
}

/// One callable to lower: its attributes, signature, the module's declared
/// types, and which callable position it sits in.
/// `Copy` because every field is a shared reference and `lower_callable` only
/// reads them: without it `clippy::needless_pass_by_value` reads the by-value
/// parameter as a move that never happens. No `Debug`, because `syn` is pinned
/// without `extra-traits` and its nodes have none to derive from.
#[derive(Clone, Copy)]
pub(super) struct Callable<'a> {
    pub(super) attributes: &'a [syn::Attribute],
    pub(super) signature: &'a syn::Signature,
    pub(super) declared: &'a Declared,
    pub(super) kind: Kind<'a>,
}

pub(super) fn lower_callable(callable: Callable<'_>) -> Result<ir::Function> {
    let Callable {
        attributes,
        signature,
        declared,
        kind,
    } = callable;
    reject_unsupported(signature)?;
    let asyncness = match signature.asyncness {
        Some(token) => {
            if matches!(kind, Kind::Constructor { .. }) {
                return Err(LowerError::new(
                    token.span(),
                    "Python constructors are synchronous; mark this \
                     #[unibind(associated)] instead, which may be async and \
                     keeps its own name",
                ));
            }
            ir::Asyncness::Async
        }
        None => ir::Asyncness::Sync,
    };

    let meta = attrs::UnibindMeta::from_attrs(attributes)?;
    meta.reject_non_callable_options(kind.context())?;
    match kind {
        // A `constructor` or `associated` flag routed the signature here
        // already, so only the other kinds can carry one by mistake.
        Kind::Free | Kind::Method => {
            meta.reject_constructor(kind.context())?;
            meta.reject_associated(kind.context())?;
        }
        Kind::Constructor { .. } => {
            meta.reject_associated(kind.context())?;
            meta.reject_blocking(kind.context())?;
        }
        // An associated function is an ordinary callable that happens to
        // hang off the type, so `blocking` applies to it exactly as it
        // does to a method.
        Kind::Associated { .. } => meta.reject_constructor(kind.context())?,
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
        Kind::Constructor { object } => {
            ret::lower_ctor_return(&signature.output, object, declared)?
        }
        // `Self` resolves here, where the enclosing impl is known; the
        // shared type lowering has no notion of one. Everything else falls
        // through to the ordinary path, so an associated function may
        // return the object, a record, a list, or nothing at all.
        Kind::Associated { object } => {
            ret::lower_associated_return(&signature.output, object, declared)?
        }
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
    meta.reject_jvm_base("an argument")?;
    meta.reject_export_options("an argument")?;
    meta.reject_resource("an argument")?;
    meta.reject_constructor("an argument")?;
    meta.reject_blocking("an argument")?;
    meta.reject_rename_all("an argument")?;
    let ty = lower_type(&arg.ty, declared, Position::Arg)?;
    // Refused here rather than in each backend: a default is a `Literal`,
    // and no backend can spell an enum variant as one. The ts glue would
    // substitute a wire string where the user's function takes the enum, and
    // the pyo3 signature would put a `&str` where the parameter is the enum
    // -- two different compile errors pointing at generated code, for one
    // shape that is easy to name here.
    if meta.default.is_some()
        && let ir::Type::Named(name) = &ty
        && declared.enums.iter().any(|declared| declared == name)
    {
        return Err(LowerError::new(
            pattern.ident.span(),
            format!(
                "argument `{}` cannot carry a default: `{name}` is a \
                 #[unibind::enumeration], and a default is a literal no \
                 backend can spell as a variant. Take `Option<{name}>` and \
                 pick the fallback in the body.",
                pattern.ident,
            ),
        ));
    }
    Ok(ir::Arg {
        name: pattern.ident.to_string(),
        names: meta.names(),
        ty,
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
