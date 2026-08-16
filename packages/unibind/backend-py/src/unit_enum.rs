//! Render a unit enum as a Python `enum.StrEnum`.
//!
//! `StrEnum` is the one Python shape that is both a real enum member and a
//! real `str`, so `status is MachineStatus.RUNNING`, `isinstance(status, str)`
//! and a string comparison a caller already wrote (`status == "running"`) are
//! all true of the same value. Nothing else on offer is: a plain `Enum` breaks
//! every existing string comparison, and a bare `str` gives up the closed set
//! the Rust side knows about.
//!
//! The class is built by the extension itself at `#[pymodule]` init, through
//! the `enum` module's functional API, so the members and their values come
//! from the same IR as the `.pyi` stub and no Python source file can drift
//! from the Rust. Each class is stashed in a `OnceLock` the outbound
//! conversion reads; nothing can call an export before `PyInit_` has run, so
//! the cell is filled by the time anything reads it.

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use unibind_core::casing::screaming_snake_case;
use unibind_core::ir;
use unibind_core::render::{RenderError, name_ident};

/// The `OnceLock` holding one enum's Python class.
fn class_cell_ident(name: &str) -> Ident {
    format_ident!("__UNIBIND_ENUM_{}", name)
}

/// The function that builds one enum's Python class and fills its cell.
fn build_ident(name: &str) -> Ident {
    format_ident!("__unibind_build_enum_{}", name)
}

/// The function turning a wire string back into a member of the class.
fn member_ident(name: &str) -> Ident {
    format_ident!("__unibind_enum_member_{}", name)
}

/// Everything one enum's generated code is spelled from, computed once so
/// the plumbing and the conversions cannot disagree about a name.
struct Spelling {
    /// The user's Rust enum, reached through the glue module's `super::`.
    rust_name: Ident,
    /// The `OnceLock` holding the Python class.
    cell: Ident,
    /// The class builder run at module init.
    build: Ident,
    /// The wire-string-to-member minter both `IntoPyObject` impls call.
    mint: Ident,
    /// Rust variant identifiers, in declaration order.
    variants: Vec<Ident>,
    /// Wire strings, index-aligned with `variants`.
    wires: Vec<String>,
    /// Python member identifiers, index-aligned with `variants`.
    members: Vec<String>,
}

impl Spelling {
    fn new(declared: &ir::Enum) -> Result<Self, RenderError> {
        Ok(Self {
            rust_name: name_ident(&declared.name)?,
            cell: class_cell_ident(&declared.name),
            build: build_ident(&declared.name),
            mint: member_ident(&declared.name),
            variants: declared
                .variants
                .iter()
                .map(|variant| name_ident(&variant.name))
                .collect::<Result<Vec<_>, _>>()?,
            wires: declared
                .variants
                .iter()
                .map(|variant| variant.wire.clone())
                .collect(),
            members: declared.variants.iter().map(member_name).collect(),
        })
    }
}

/// The cell, the builder, the member minter, and the two pyo3 conversions
/// for one unit enum.
///
/// # Errors
///
/// Fails for an enum or variant name that cannot become a Rust identifier.
pub fn render_enum(declared: &ir::Enum, user: &Ident) -> Result<TokenStream, RenderError> {
    let spelling = Spelling::new(declared)?;
    let plumbing = plumbing(declared, &spelling);
    let conversions = conversions(declared, &spelling, user);
    Ok(quote! {
        #plumbing
        #conversions
    })
}

/// The class cell, the builder that fills it, and the minter that reads it.
fn plumbing(declared: &ir::Enum, spelling: &Spelling) -> TokenStream {
    let Spelling {
        cell,
        build,
        mint,
        wires,
        members,
        ..
    } = spelling;
    let class_name = class_name(declared);
    let doc = declared.docs.join("\n");
    let unregistered = format!(
        "the {class_name} class is not registered; this is only reachable if \
         the extension module's initializer did not run"
    );
    quote! {
        /// The generated `enum.StrEnum` class, filled at module init.
        static #cell: ::std::sync::OnceLock<::pyo3::Py<::pyo3::PyAny>> =
            ::std::sync::OnceLock::new();

        /// Build the `StrEnum` class through the `enum` module's functional
        /// API and hand it back for registration. The cell keeps a reference
        /// so the outbound conversion can mint members without importing the
        /// generated package, which imports this extension in turn.
        #[allow(dead_code)]
        fn #build<'py>(
            py: ::pyo3::Python<'py>,
            module_name: &str,
        ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
            let enum_module = ::pyo3::Python::import(py, "enum")?;
            let str_enum = ::pyo3::types::PyAnyMethods::getattr(
                ::pyo3::Bound::as_any(&enum_module),
                "StrEnum",
            )?;
            let members: ::std::vec::Vec<(&str, &str)> =
                ::std::vec![#((#members, #wires)),*];
            let options = ::pyo3::types::PyDict::new(py);
            ::pyo3::types::PyDictMethods::set_item(&options, "module", module_name)?;
            ::pyo3::types::PyDictMethods::set_item(&options, "qualname", #class_name)?;
            let class = ::pyo3::types::PyAnyMethods::call(
                &str_enum,
                (#class_name, members),
                ::std::option::Option::Some(&options),
            )?;
            ::pyo3::types::PyAnyMethods::setattr(&class, "__doc__", #doc)?;
            let _ = #cell.set(::pyo3::Bound::unbind(::std::clone::Clone::clone(&class)));
            ::pyo3::PyResult::Ok(class)
        }

        /// The class member whose value is `wire`. Both `IntoPyObject` impls
        /// go through here, so the owned and borrowed halves cannot answer
        /// differently.
        #[allow(dead_code)]
        fn #mint<'py>(
            wire: &str,
            py: ::pyo3::Python<'py>,
        ) -> ::pyo3::PyResult<::pyo3::Bound<'py, ::pyo3::PyAny>> {
            let class = #cell.get().ok_or_else(|| {
                ::pyo3::exceptions::PyRuntimeError::new_err(#unregistered)
            })?;
            ::pyo3::types::PyAnyMethods::call1(::pyo3::Py::bind(class, py), (wire,))
        }
    }
}

/// The three pyo3 conversions: one inbound, and one outbound per receiver
/// shape (a record's `#[pyo3(get)]` getter converts through a reference).
fn conversions(declared: &ir::Enum, spelling: &Spelling, user: &Ident) -> TokenStream {
    let Spelling {
        rust_name,
        mint,
        variants,
        wires,
        ..
    } = spelling;
    let rejection = format!(
        "is not a {}; expected one of {}",
        class_name(declared),
        wires.join(", ")
    );
    // One `match` text, spelled once and used by both outbound impls.
    let outbound = quote! {
        #mint(
            match self {
                #(super::#user::#rust_name::#variants => #wires,)*
            },
            py,
        )
    };
    quote! {
        // Inbound accepts the member and the bare string alike: a `StrEnum`
        // member IS a `str`, so one extraction covers both and a caller who
        // never imported the class keeps working. A word outside the set is
        // refused by name with the set spelled out -- the same `ValueError`
        // Python's own `enum` raises for an unknown value.
        impl<'a, 'py> ::pyo3::FromPyObject<'a, 'py> for super::#user::#rust_name {
            type Error = ::pyo3::PyErr;

            fn extract(
                value: ::pyo3::Borrowed<'a, 'py, ::pyo3::PyAny>,
            ) -> ::std::result::Result<Self, Self::Error> {
                let text: ::std::string::String =
                    ::pyo3::types::PyAnyMethods::extract(&*value)?;
                match text.as_str() {
                    #(#wires => ::std::result::Result::Ok(
                        super::#user::#rust_name::#variants,
                    ),)*
                    other => ::std::result::Result::Err(
                        ::pyo3::exceptions::PyValueError::new_err(
                            ::std::format!("`{}` {}", other, #rejection),
                        ),
                    ),
                }
            }
        }

        impl<'py> ::pyo3::IntoPyObject<'py> for super::#user::#rust_name {
            type Target = ::pyo3::PyAny;
            type Output = ::pyo3::Bound<'py, ::pyo3::PyAny>;
            type Error = ::pyo3::PyErr;

            fn into_pyobject(
                self,
                py: ::pyo3::Python<'py>,
            ) -> ::std::result::Result<Self::Output, Self::Error> {
                #outbound
            }
        }

        impl<'py> ::pyo3::IntoPyObject<'py> for &super::#user::#rust_name {
            type Target = ::pyo3::PyAny;
            type Output = ::pyo3::Bound<'py, ::pyo3::PyAny>;
            type Error = ::pyo3::PyErr;

            fn into_pyobject(
                self,
                py: ::pyo3::Python<'py>,
            ) -> ::std::result::Result<Self::Output, Self::Error> {
                #outbound
            }
        }
    }
}

/// The statement registering one enum's class on the `#[pymodule]`.
///
/// `module_name` is the extension module's Python name, which becomes the
/// class's `__module__`: that is what makes the members picklable and what
/// `repr` and `help` show, so it has to be the module the class is reachable
/// from rather than the class's own name.
pub fn registration(declared: &ir::Enum, module_name: &str) -> TokenStream {
    let build = build_ident(&declared.name);
    let class_name = class_name(declared);
    quote! {
        {
            let class = #build(module.py(), #module_name)?;
            module.add(#class_name, class)?;
        }
    }
}

/// The Python class name: the `py` rename when set, the Rust name otherwise.
/// Same rule every other declared type follows.
fn class_name(declared: &ir::Enum) -> &str {
    declared.names.py.as_deref().unwrap_or(&declared.name)
}

/// A variant's Python member identifier. Lowering fills it in
/// (`SCREAMING_SNAKE_CASE` unless `py(name = ...)` overrides), so the fallback
/// only covers IR read back from an artifact some other producer wrote, and it
/// applies the same rule rather than a second one.
fn member_name(variant: &ir::EnumVariant) -> String {
    variant
        .names
        .py
        .clone()
        .unwrap_or_else(|| screaming_snake_case(&variant.name))
}
