//! The `<Ns>.Native` module: `@on_load` NIF loading and one stub per NIF.

use std::fmt::Write as _;
use std::path::Path;

use unibind_core::ir;

use crate::{module, names};

/// Render the `<Ns>.Native` module. `nif_soname` is the built library's
/// file name; the load path drops its extension.
pub fn render(interface: &ir::Interface, nif_soname: &str) -> String {
    let ns = names::ns_name(interface);
    let ns_snake = names::ns_snake(interface);
    let lib_stem = Path::new(nif_soname).file_stem().map_or_else(
        || nif_soname.to_owned(),
        |stem| stem.to_string_lossy().into_owned(),
    );

    let mut out = String::new();
    let _ = writeln!(out, "defmodule {ns}.Native do");
    out.push_str("  @moduledoc false\n\n");
    let _ = writeln!(out, "  @app :{ns_snake}");
    out.push_str("\n  @on_load :__load_nif__\n\n");
    out.push_str("  def __load_nif__ do\n");
    out.push_str("    :code.priv_dir(@app)\n");
    out.push_str("    |> to_string()\n");
    let _ = writeln!(out, "    |> Path.join(\"native/{lib_stem}\")");
    out.push_str("    |> String.to_charlist()\n");
    out.push_str("    |> :erlang.load_nif(0)\n");
    out.push_str("  end\n");

    for function in &interface.functions {
        stub(
            &mut out,
            &names::ex_fn_name(function),
            &stub_args(function, Leading::None),
        );
    }
    for object in &interface.objects {
        if let Some(constructor) = &object.constructor {
            stub(
                &mut out,
                &names::member_nif_name(object, constructor),
                &stub_args(constructor, Leading::None),
            );
        }
        for method in &object.methods {
            stub(
                &mut out,
                &names::member_nif_name(object, method),
                &stub_args(method, Leading::Handle),
            );
        }
    }
    if module::has_streams(interface) {
        stub(
            &mut out,
            "unibind_demand",
            &["_handle".to_owned(), "_n".to_owned()],
        );
    }
    out.push_str("end\n");
    out
}

/// The extra first stub argument, before the declared ones.
#[derive(Clone, Copy)]
enum Leading {
    None,
    Handle,
}

/// Underscored placeholder names matching the NIF's Elixir-visible arity:
/// async and stream functions take the reply reference first, methods take
/// the resource handle first.
fn stub_args(function: &ir::Function, leading: Leading) -> Vec<String> {
    let mut args = Vec::new();
    if matches!(leading, Leading::Handle) {
        args.push("_handle".to_owned());
    }
    if matches!(function.asyncness, ir::Asyncness::Async)
        || matches!(function.ret, Some(ir::Type::Stream(_)))
    {
        args.push("_ref".to_owned());
    }
    for arg in &function.args {
        args.push(format!("_{}", names::ex_arg_name(arg)));
    }
    args
}

fn stub(out: &mut String, name: &str, args: &[String]) {
    out.push('\n');
    if args.is_empty() {
        let _ = writeln!(out, "  def {name}, do: :erlang.nif_error(:not_loaded)");
    } else {
        let _ = writeln!(
            out,
            "  def {name}({}), do: :erlang.nif_error(:not_loaded)",
            args.join(", ")
        );
    }
}
