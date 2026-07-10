//! The resource surface of an object: idempotent `close()`, `async with`
//! support, and a `ResourceWarning` when a handle leaks unclosed.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use unibind_core::ir;

use crate::function::doc_attrs;

/// The generated `close`, `__aenter__`, and `__aexit__` pymethods.
pub fn surface(object: &ir::Object) -> TokenStream {
    let close = user_close(object);
    let close_wrapper = close_method(close);
    let aenter = aenter();
    let aexit = aexit(close);
    quote! {
        #close_wrapper
        #aenter
        #aexit
    }
}

fn user_close(object: &ir::Object) -> &ir::Function {
    object
        .methods
        .iter()
        .find(|method| method.name == "close" && method.args.is_empty() && method.ret.is_none())
        .expect("lowering guarantees resources declare close()")
}

/// The statement invoking the user's close on `receiver`, awaiting and
/// error-mapping as its shape demands.
fn close_stmt(close: &ir::Function, receiver: &TokenStream) -> TokenStream {
    let call = match close.asyncness {
        ir::Asyncness::Async => quote!(#receiver.close().await),
        ir::Asyncness::Sync => quote!(#receiver.close()),
    };
    if close.throws.is_some() {
        quote!(#call.map_err(::pyo3::PyErr::from)?;)
    } else {
        quote!(#call;)
    }
}

/// `close()` is idempotent: `swap` picks one winner between racing
/// `close()` / `__aexit__` calls, so the user's close runs exactly once.
fn close_method(close: &ir::Function) -> TokenStream {
    let docs = doc_attrs(&close.docs);
    match close.asyncness {
        ir::Asyncness::Sync => {
            let receiver = quote!(self.inner);
            let stmt = close_stmt(close, &receiver);
            quote! {
                #docs
                fn close(&self) -> ::pyo3::PyResult<()> {
                    if self.closed.swap(true, ::std::sync::atomic::Ordering::SeqCst) {
                        return ::pyo3::PyResult::Ok(());
                    }
                    #stmt
                    ::pyo3::PyResult::Ok(())
                }
            }
        }
        // An async user close returns a coroutine; whether the user's
        // close runs is decided before the future is built, so a second
        // call awaits to a no-op.
        ir::Asyncness::Async => {
            let receiver = quote!(inner);
            let stmt = close_stmt(close, &receiver);
            quote! {
                #docs
                fn close<'py>(
                    &self,
                    py: ::pyo3::Python<'py>,
                ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
                    let first = !self.closed.swap(true, ::std::sync::atomic::Ordering::SeqCst);
                    let inner = ::std::sync::Arc::clone(&self.inner);
                    ::unibind_py_runtime::future_into_py(py, async move {
                        if first {
                            #stmt
                        }
                        ::pyo3::PyResult::Ok(())
                    })
                }
            }
        }
    }
}

fn aenter() -> TokenStream {
    quote! {
        #[doc = "Enter `async with`: resolves to the object itself."]
        fn __aenter__<'py>(
            slf: ::pyo3::Bound<'py, Self>,
        ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
            let py = slf.py();
            let owned: ::pyo3::Py<Self> = slf.unbind();
            ::unibind_py_runtime::future_into_py(py, async move { ::pyo3::PyResult::Ok(owned) })
        }
    }
}

/// `__aexit__` closes once (sharing the flag with `close()`) and resolves
/// to `false` so the in-flight exception is never suppressed.
fn aexit(close: &ir::Function) -> TokenStream {
    let receiver = quote!(inner);
    let stmt = close_stmt(close, &receiver);
    quote! {
        #[doc = "Exit `async with`: closes the resource, never suppresses the exception."]
        fn __aexit__<'py>(
            &self,
            py: ::pyo3::Python<'py>,
            _exc_type: ::pyo3::Bound<'py, ::pyo3::PyAny>,
            _exc: ::pyo3::Bound<'py, ::pyo3::PyAny>,
            _tb: ::pyo3::Bound<'py, ::pyo3::PyAny>,
        ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
            let first = !self.closed.swap(true, ::std::sync::atomic::Ordering::SeqCst);
            let inner = ::std::sync::Arc::clone(&self.inner);
            ::unibind_py_runtime::future_into_py(py, async move {
                if first {
                    #stmt
                }
                ::pyo3::PyResult::Ok(false)
            })
        }
    }
}

/// A `Drop` that warns when the resource was never closed. Mirrors
/// `CPython`'s own unclosed-socket `ResourceWarning`, so leaks surface under
/// `python -W error::ResourceWarning` and pytest `filterwarnings`.
pub fn leak_warning(object: &ir::Object) -> TokenStream {
    let wrapper = crate::ctx::object_wrapper_ident(&object.name);
    let py_name = object.names.py.as_deref().unwrap_or(&object.name);
    let text = format!("unclosed {py_name}: call close() or use 'async with'");
    let message = syn::LitCStr::new(
        &std::ffi::CString::new(text).expect("class names contain no NUL"),
        Span::call_site(),
    );
    // `try_attach`, not `attach`: drops can run while the interpreter
    // shuts down, where attaching would panic, and a Drop must not panic.
    // Both Results are swallowed for the same reason (`warn` errors under
    // warnings-as-errors).
    quote! {
        impl ::std::ops::Drop for #wrapper {
            fn drop(&mut self) {
                if self.closed.load(::std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                let _ = ::pyo3::Python::try_attach(|py| {
                    let category = py.get_type::<::pyo3::exceptions::PyResourceWarning>();
                    let _ = ::pyo3::PyErr::warn(py, category.as_any(), #message, 1);
                });
            }
        }
    }
}
