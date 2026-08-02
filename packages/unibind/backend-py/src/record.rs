//! Attach `#[pyclass]` to record structs and render their constructors.

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use syn::parse_quote;
use unibind_core::ir;
use unibind_core::render::{self, RenderError, RenderedRecord};

use crate::function::doc_attrs;

/// The attributes the exported struct gains: `#[pyclass]` on the item and a
/// read-only getter per field.
pub fn record_attrs(record: &ir::Record) -> RenderedRecord {
    let outer: syn::Attribute = record.names.py.as_ref().map_or_else(
        || parse_quote!(#[::pyo3::pyclass(from_py_object)]),
        |name| parse_quote!(#[::pyo3::pyclass(from_py_object, name = #name)]),
    );
    let fields = record
        .fields
        .iter()
        .map(|field| {
            let attr: syn::Attribute = field.names.py.as_ref().map_or_else(
                || parse_quote!(#[pyo3(get)]),
                |name| parse_quote!(#[pyo3(get, name = #name)]),
            );
            vec![attr]
        })
        .collect();
    RenderedRecord {
        outer: vec![outer],
        fields,
    }
}

/// A `#[pymethods]` block giving ordinary value records a positional
/// constructor and all-optional option records a keyword-only constructor.
/// Optional fields default to `None`, matching the TypeScript record surface.
///
/// The block also carries the two methods that make a record livable in a
/// notebook: a `__repr__` showing every field by its Python name (the pyclass
/// default is `<builtins.Vm object at 0x..>`, which hides everything), and a
/// shallow `to_dict` so a list of records feeds `pandas.DataFrame` without a
/// per-field comprehension. Both convert through the same `IntoPyObject`
/// machinery the `#[pyo3(get)]` getters already require, so they impose no
/// new bound on field types.
pub fn constructor(record: &ir::Record, user: &Ident) -> Result<TokenStream, RenderError> {
    let name = Ident::new(&record.name, Span::call_site());
    let class_name = record.names.py.clone().unwrap_or_else(|| record.name.clone());
    let mut params = Vec::new();
    let mut field_idents = Vec::new();
    let mut plain_idents = Vec::new();
    let mut py_names = Vec::new();
    let mut signature = Vec::new();
    for (index, field) in record.fields.iter().enumerate() {
        let ident = Ident::new(&field.name, Span::call_site());
        let py_ident = render::name_ident(field.names.py.as_ref().unwrap_or(&field.name))?;
        let ty = render::rust_type(&field.ty, user, render::Ownership::Declared);
        plain_idents.push(ident.clone());
        py_names.push(field.names.py.clone().unwrap_or_else(|| field.name.clone()));
        params.push(quote!(#py_ident: #ty));
        if matches!(field.ty, ir::Type::Option(_))
            && record.fields[index..]
                .iter()
                .all(|field| matches!(field.ty, ir::Type::Option(_)))
        {
            signature.push(quote!(#py_ident = None));
        } else {
            signature.push(quote!(#py_ident));
        }
        field_idents.push(quote!(#ident: #py_ident));
    }
    let docs = doc_attrs(&record.docs);
    let signature = if !record.fields.is_empty()
        && record
            .fields
            .iter()
            .all(|field| matches!(field.ty, ir::Type::Option(_)))
    {
        quote!(*, #(#signature),*)
    } else {
        quote!(#(#signature),*)
    };
    Ok(quote! {
        #[::pyo3::pymethods]
        impl super::#user::#name {
            #docs
            #[new]
            #[pyo3(signature = (#signature))]
            fn __unibind_new(#(#params),*) -> Self {
                Self {
                    #(#field_idents),*
                }
            }

            #[allow(unused_variables)]
            fn __repr__(&self, py: ::pyo3::Python<'_>) -> ::pyo3::PyResult<::std::string::String> {
                let mut parts: ::std::vec::Vec<::std::string::String> = ::std::vec::Vec::new();
                #(
                    parts.push(::std::format!(
                        "{}={}",
                        #py_names,
                        ::pyo3::types::PyStringMethods::to_string_lossy(
                            &::pyo3::types::PyAnyMethods::repr(
                                ::pyo3::Py::bind(
                                    &::pyo3::IntoPyObjectExt::into_py_any(
                                        self.#plain_idents.clone(),
                                        py,
                                    )?,
                                    py,
                                ),
                            )?,
                        )
                    ));
                )*
                ::pyo3::PyResult::Ok(::std::format!("{}({})", #class_name, parts.join(", ")))
            }

            /// Shallow dict of this record's fields, keyed by their Python
            /// names. Nested records stay objects; call `to_dict` on them to
            /// go deeper.
            #[allow(unused_variables)]
            fn to_dict<'py>(
                &self,
                py: ::pyo3::Python<'py>,
            ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::types::PyDict>> {
                let dict = ::pyo3::types::PyDict::new(py);
                #(
                    ::pyo3::types::PyDictMethods::set_item(
                        &dict,
                        #py_names,
                        ::pyo3::IntoPyObjectExt::into_py_any(self.#plain_idents.clone(), py)?,
                    )?;
                )*
                ::pyo3::PyResult::Ok(dict)
            }

            // The read-only mapping protocol, so a record IS dict-like:
            // `dict(record)`, `{**record}`, and
            // `pandas.DataFrame(records)` all work without a comprehension.
            // The generated package registers every record with
            // `collections.abc.Mapping`, so the full read-only surface is
            // implemented here, not just the three methods duck-typing
            // checks reach for.

            /// The field names, in declaration order.
            fn keys(&self) -> ::std::vec::Vec<&'static str> {
                ::std::vec![#(#py_names),*]
            }

            /// The field values, in declaration order.
            #[allow(unused_variables)]
            fn values(
                &self,
                py: ::pyo3::Python<'_>,
            ) -> ::pyo3::PyResult<::std::vec::Vec<::pyo3::Py<::pyo3::PyAny>>> {
                ::pyo3::PyResult::Ok(::std::vec![
                    #(::pyo3::IntoPyObjectExt::into_py_any(self.#plain_idents.clone(), py)?),*
                ])
            }

            /// `(name, value)` pairs, in declaration order.
            #[allow(unused_variables)]
            fn items(
                &self,
                py: ::pyo3::Python<'_>,
            ) -> ::pyo3::PyResult<::std::vec::Vec<(&'static str, ::pyo3::Py<::pyo3::PyAny>)>> {
                ::pyo3::PyResult::Ok(::std::vec![
                    #((
                        #py_names,
                        ::pyo3::IntoPyObjectExt::into_py_any(self.#plain_idents.clone(), py)?,
                    )),*
                ])
            }

            /// The field named `key`, or `default` (`None` unset) when the
            /// record has no such field.
            #[allow(unused_variables)]
            #[pyo3(signature = (key, default = None))]
            fn get(
                &self,
                py: ::pyo3::Python<'_>,
                key: &str,
                default: ::std::option::Option<::pyo3::Py<::pyo3::PyAny>>,
            ) -> ::pyo3::PyResult<::std::option::Option<::pyo3::Py<::pyo3::PyAny>>> {
                match key {
                    #(#py_names => ::pyo3::PyResult::Ok(::std::option::Option::Some(
                        ::pyo3::IntoPyObjectExt::into_py_any(self.#plain_idents.clone(), py)?,
                    )),)*
                    _ => ::pyo3::PyResult::Ok(default),
                }
            }

            #[allow(unused_variables)]
            fn __getitem__(
                &self,
                py: ::pyo3::Python<'_>,
                key: &str,
            ) -> ::pyo3::PyResult<::pyo3::Py<::pyo3::PyAny>> {
                match key {
                    #(#py_names => ::pyo3::IntoPyObjectExt::into_py_any(
                        self.#plain_idents.clone(),
                        py,
                    ),)*
                    _ => ::pyo3::PyResult::Err(
                        ::pyo3::exceptions::PyKeyError::new_err(key.to_owned()),
                    ),
                }
            }

            fn __contains__(&self, key: &str) -> bool {
                [#(#py_names),*].contains(&key)
            }

            fn __len__(&self) -> usize {
                [#(#py_names),*].len()
            }

            /// Iterates the field names, like a dict.
            fn __iter__<'py>(
                &self,
                py: ::pyo3::Python<'py>,
            ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::types::PyIterator>> {
                let keys = ::pyo3::Bound::into_any(
                    ::pyo3::types::PyList::new(py, [#(#py_names),*])?,
                );
                ::pyo3::types::PyAnyMethods::try_iter(&keys)
            }
        }
    })
}
