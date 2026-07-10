//! The `<Ns>` wrapper module: record and error structs, object modules,
//! and the typespec'd public functions.

use std::fmt::Write as _;

use heck::ToSnakeCase as _;
use unibind_core::ir;

use crate::host::{calls, typespec};
use crate::{module, names};

/// Render the `<Ns>` wrapper module. Validation happens in
/// [`crate::host_modules`], which rejects everything the glue renderer
/// would.
pub fn render(interface: &ir::Interface) -> String {
    let ns = names::ns_name(interface);
    let mut out = String::new();
    let _ = writeln!(out, "defmodule {ns} do");
    if interface.docs.is_empty() {
        out.push_str("  @moduledoc false\n");
    } else {
        calls::doc_kind(&mut out, "moduledoc", &interface.docs, "  ");
    }
    let _ = writeln!(out, "\n  alias {ns}.Native");

    for record in &interface.records {
        render_record(&mut out, record, interface, &ns);
    }
    for error in &interface.errors {
        render_error(&mut out, error);
    }
    for object in &interface.objects {
        render_object(&mut out, object, interface);
    }
    for function in &interface.functions {
        out.push('\n');
        let target = calls::Target {
            nif_name: names::ex_fn_name(function),
            ex_name: names::ex_fn_name(function),
            handle_param: None,
            ret_override: None,
        };
        calls::render_fn(&mut out, function, &target, interface, &ns, "  ");
    }
    if module::has_streams(interface) {
        calls::stream_helper(&mut out);
    }
    out.push_str("end\n");
    out
}

/// A record: `@enforce_keys` + `defstruct` + a `t()` mapping each field to
/// its typespec. Field keys are the Rust field names (rustler's `NifStruct`
/// derives its atoms from them).
fn render_record(out: &mut String, record: &ir::Record, interface: &ir::Interface, ns: &str) {
    let name = names::ex_record_name(record);
    let _ = writeln!(out, "\n  defmodule {name} do");
    if record.docs.is_empty() {
        out.push_str("    @moduledoc false\n");
    } else {
        calls::doc_kind(out, "moduledoc", &record.docs, "    ");
    }
    let keys: Vec<String> = record
        .fields
        .iter()
        .map(|field| format!(":{}", field.name))
        .collect();
    let _ = writeln!(out, "\n    @enforce_keys [{}]", keys.join(", "));
    let _ = writeln!(out, "    defstruct [{}]", keys.join(", "));
    let field_specs: Vec<String> = record
        .fields
        .iter()
        .map(|field| {
            format!(
                "{}: {}",
                field.name,
                typespec::typespec(&field.ty, interface, ns)
            )
        })
        .collect();
    let _ = writeln!(
        out,
        "    @type t :: %__MODULE__{{{}}}",
        field_specs.join(", ")
    );
    out.push_str("  end\n");
}

/// An error: a plain struct of `variant` (one atom per Rust variant) and
/// the `Display` text.
fn render_error(out: &mut String, error: &ir::ErrorType) {
    let name = names::ex_error_name(error);
    let _ = writeln!(out, "\n  defmodule {name} do");
    if error.docs.is_empty() {
        out.push_str("    @moduledoc false\n");
    } else {
        calls::doc_kind(out, "moduledoc", &error.docs, "    ");
    }
    let atoms: Vec<String> = error
        .variants
        .iter()
        .map(|variant| format!(":{}", names::variant_atom(variant)))
        .collect();
    out.push_str("\n    defstruct [:variant, :message]\n");
    let _ = writeln!(
        out,
        "    @type t :: %__MODULE__{{variant: {}, message: String.t()}}",
        atoms.join(" | ")
    );
    out.push_str("  end\n");
}

/// An object: an opaque `reference()` handle with one function per
/// constructor and method.
fn render_object(out: &mut String, object: &ir::Object, interface: &ir::Interface) {
    let ns = names::ns_name(interface);
    let name = names::ex_object_name(object);
    let _ = writeln!(out, "\n  defmodule {name} do");
    if object.docs.is_empty() {
        out.push_str("    @moduledoc false\n");
    } else {
        calls::doc_kind(out, "moduledoc", &object.docs, "    ");
    }
    let _ = writeln!(
        out,
        "\n    @typedoc \"An opaque handle to a Rust `{}`.\"",
        object.name
    );
    out.push_str("    @type t :: reference()\n");
    let handle = name.to_snake_case();
    if let Some(constructor) = &object.constructor {
        out.push('\n');
        let target = calls::Target {
            nif_name: names::member_nif_name(object, constructor),
            ex_name: names::ex_fn_name(constructor),
            handle_param: None,
            ret_override: Some("t()".to_owned()),
        };
        calls::render_fn(out, constructor, &target, interface, &ns, "    ");
    }
    for method in &object.methods {
        out.push('\n');
        let target = calls::Target {
            nif_name: names::member_nif_name(object, method),
            ex_name: names::ex_fn_name(method),
            handle_param: Some(&handle),
            ret_override: None,
        };
        calls::render_fn(out, method, &target, interface, &ns, "    ");
    }
    out.push_str("  end\n");
}
