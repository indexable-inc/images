//! Render the wrapper module's function definitions: sync delegation,
//! async receive loops, and stream construction.

use std::fmt::Write as _;

use unibind_core::ir;

use crate::host::typespec;
use crate::names;

/// How a wrapper function reaches its NIF.
pub struct Target<'a> {
    /// The registered NIF name (`rows`, `counter_new`).
    pub nif_name: String,
    /// The Elixir-facing function name.
    pub ex_name: String,
    /// `Some` with the parameter name when the NIF takes a leading handle.
    pub handle_param: Option<&'a str>,
    /// Overrides the success typespec: a constructor's IR return names the
    /// object, but on the Elixir side it is the opaque `t()`.
    pub ret_override: Option<String>,
}

/// Append `@doc` + `@spec` + `def` for one function, `indent` levels deep.
pub fn render_fn(
    out: &mut String,
    function: &ir::Function,
    target: &Target<'_>,
    interface: &ir::Interface,
    ns: &str,
    pad: &str,
) {
    doc(out, &function.docs, pad);
    let error_spec = function
        .throws
        .as_ref()
        .map(|throws| format!("{ns}.{}.t()", names::ex_error_name_of(interface, throws)));
    let is_stream = matches!(function.ret, Some(ir::Type::Stream(_)));

    let mut param_specs: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();
    let mut forwards: Vec<String> = Vec::new();
    if let Some(handle) = target.handle_param {
        param_specs.push("t()".to_owned());
        params.push(handle.to_owned());
        forwards.push(handle.to_owned());
    }
    for arg in &function.args {
        param_specs.push(typespec::typespec(&arg.ty, interface, ns));
        let name = names::ex_arg_name(arg);
        forwards.push(name.clone());
        params.push(match (&arg.default, &arg.ty) {
            (Some(default), _) => format!("{name} \\\\ {}", typespec::literal(default)),
            (None, ir::Type::Option(_)) => format!("{name} \\\\ nil"),
            (None, _) => name,
        });
    }

    let ok_spec = if is_stream {
        Some("Enumerable.t()".to_owned())
    } else {
        target.ret_override.clone().or_else(|| {
            function
                .ret
                .as_ref()
                .map(|ret| typespec::typespec(ret, interface, ns))
        })
    };
    let ret = match (error_spec, ok_spec) {
        (None, None) => ":ok".to_owned(),
        (None, Some(ok)) => ok,
        (Some(error), None) => format!(":ok | {{:error, {error}}}"),
        (Some(error), Some(ok)) => format!("{{:ok, {ok}}} | {{:error, {error}}}"),
    };
    let _ = writeln!(
        out,
        "{pad}@spec {}({}) :: {ret}",
        target.ex_name,
        param_specs.join(", ")
    );
    let _ = writeln!(out, "{pad}def {}({}) do", target.ex_name, params.join(", "));
    if matches!(function.asyncness, ir::Asyncness::Async) {
        async_body(out, function, target, &forwards, pad);
    } else if is_stream {
        stream_body(out, function, target, &forwards, pad);
    } else {
        sync_body(out, function, target, &forwards, pad);
    }
    let _ = writeln!(out, "{pad}end");
}

fn call(target: &Target<'_>, forwards: &[String]) -> String {
    format!("Native.{}({})", target.nif_name, forwards.join(", "))
}

fn call_with_ref(target: &Target<'_>, forwards: &[String]) -> String {
    let mut with_ref = vec!["ref".to_owned()];
    with_ref.extend_from_slice(forwards);
    call(target, &with_ref)
}

fn sync_body(
    out: &mut String,
    function: &ir::Function,
    target: &Target<'_>,
    forwards: &[String],
    pad: &str,
) {
    let call = call(target, forwards);
    let has_value = function.ret.is_some() || target.ret_override.is_some();
    if has_value {
        // Values (and `{:ok, _} | {:error, _}` results) pass straight through.
        let _ = writeln!(out, "{pad}  {call}");
    } else if function.throws.is_some() {
        let _ = writeln!(out, "{pad}  case {call} do");
        let _ = writeln!(out, "{pad}    {{:ok, _}} -> :ok");
        let _ = writeln!(out, "{pad}    {{:error, error}} -> {{:error, error}}");
        let _ = writeln!(out, "{pad}  end");
    } else {
        let _ = writeln!(out, "{pad}  {call}");
        let _ = writeln!(out, "{pad}  :ok");
    }
}

fn async_body(
    out: &mut String,
    function: &ir::Function,
    target: &Target<'_>,
    forwards: &[String],
    pad: &str,
) {
    let _ = writeln!(out, "{pad}  ref = make_ref()");
    let _ = writeln!(
        out,
        "{pad}  _inflight = {}",
        call_with_ref(target, forwards)
    );
    let _ = writeln!(out, "{pad}  receive do");
    match (&function.throws, &function.ret) {
        (None, Some(_)) => {
            let _ = writeln!(
                out,
                "{pad}    {{:unibind, ^ref, {{:ok, result}}}} -> result"
            );
        }
        (None, None) => {
            let _ = writeln!(out, "{pad}    {{:unibind, ^ref, {{:ok, _}}}} -> :ok");
        }
        (Some(_), Some(_)) => {
            let _ = writeln!(out, "{pad}    {{:unibind, ^ref, result}} -> result");
        }
        (Some(_), None) => {
            let _ = writeln!(out, "{pad}    {{:unibind, ^ref, {{:ok, _}}}} -> :ok");
            let _ = writeln!(
                out,
                "{pad}    {{:unibind, ^ref, {{:error, error}}}} -> {{:error, error}}"
            );
        }
    }
    let _ = writeln!(out, "{pad}  end");
}

fn stream_body(
    out: &mut String,
    function: &ir::Function,
    target: &Target<'_>,
    forwards: &[String],
    pad: &str,
) {
    let _ = writeln!(out, "{pad}  ref = make_ref()");
    if function.throws.is_some() {
        let _ = writeln!(out, "{pad}  case {} do", call_with_ref(target, forwards));
        let _ = writeln!(
            out,
            "{pad}    {{:ok, handle}} -> {{:ok, unibind_stream(ref, handle)}}"
        );
        let _ = writeln!(out, "{pad}    {{:error, error}} -> {{:error, error}}");
        let _ = writeln!(out, "{pad}  end");
    } else {
        let _ = writeln!(out, "{pad}  handle = {}", call_with_ref(target, forwards));
        let _ = writeln!(out, "{pad}  unibind_stream(ref, handle)");
    }
}

/// The one private helper turning a stream handle into an `Enumerable`,
/// granting one credit of demand per step.
pub fn stream_helper(out: &mut String) {
    out.push_str("\n  defp unibind_stream(ref, handle) do\n");
    out.push_str("    Stream.resource(\n");
    out.push_str("      fn -> handle end,\n");
    out.push_str("      fn handle ->\n");
    out.push_str("        Native.unibind_demand(handle, 1)\n\n");
    out.push_str("        receive do\n");
    out.push_str("          {:unibind_stream, ^ref, {:item, item}} -> {[item], handle}\n");
    out.push_str("          {:unibind_stream, ^ref, :done} -> {:halt, handle}\n");
    out.push_str("        end\n");
    out.push_str("      end,\n");
    out.push_str("      fn _handle -> :ok end\n");
    out.push_str("    )\n");
    out.push_str("  end\n");
}

/// Append a `@doc` (or `@moduledoc`) heredoc, `pad` deep.
pub fn doc(out: &mut String, lines: &[String], pad: &str) {
    doc_kind(out, "doc", lines, pad);
}

/// Append a documentation attribute heredoc when there is documentation.
pub fn doc_kind(out: &mut String, kind: &str, lines: &[String], pad: &str) {
    if lines.is_empty() {
        return;
    }
    let _ = writeln!(out, "{pad}@{kind} \"\"\"");
    for line in lines {
        if line.is_empty() {
            out.push('\n');
        } else {
            let _ = writeln!(out, "{pad}{line}");
        }
    }
    let _ = writeln!(out, "{pad}\"\"\"");
}
