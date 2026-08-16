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

    let Signature {
        specs: param_specs,
        params,
        forwards,
    } = signature(function, target, interface, ns);

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

/// A wrapper function's Elixir side: one typespec per parameter, the `def`
/// head (defaults included), and the names forwarded to the NIF.
struct Signature {
    specs: Vec<String>,
    params: Vec<String>,
    forwards: Vec<String>,
}

fn signature(
    function: &ir::Function,
    target: &Target<'_>,
    interface: &ir::Interface,
    ns: &str,
) -> Signature {
    let mut specs: Vec<String> = Vec::new();
    let mut params: Vec<String> = Vec::new();
    let mut forwards: Vec<String> = Vec::new();
    if let Some(handle) = target.handle_param {
        specs.push("t()".to_owned());
        params.push(handle.to_owned());
        forwards.push(handle.to_owned());
    }
    for arg in &function.args {
        specs.push(typespec::typespec(&arg.ty, interface, ns));
        let name = names::ex_arg_name(arg);
        forwards.push(name.clone());
        params.push(match (&arg.default, &arg.ty) {
            (Some(default), _) => format!("{name} \\\\ {}", typespec::literal(default)),
            (None, ir::Type::Option(_)) => format!("{name} \\\\ nil"),
            (None, _) => name,
        });
    }
    Signature {
        specs,
        params,
        forwards,
    }
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
    stream_call(
        out,
        function,
        target,
        forwards,
        pad,
        "unibind_stream(ref, handle)",
    );
}

/// The body both stream forms share: mint the caller's reference, call the
/// NIF with it, and wrap the handle it answers with.
///
/// `wrapped` is the Elixir expression the raw handle becomes -- an
/// `Enumerable` for the blocking form, a `StreamHandle` struct for the
/// demand-driven one -- and is the only difference between them. The `throws`
/// split has to be written once per form otherwise, and a `case` arm that only
/// appears in one of them is a difference no test would catch.
fn stream_call(
    out: &mut String,
    function: &ir::Function,
    target: &Target<'_>,
    forwards: &[String],
    pad: &str,
    wrapped: &str,
) {
    let _ = writeln!(out, "{pad}  ref = make_ref()");
    if function.throws.is_some() {
        let _ = writeln!(out, "{pad}  case {} do", call_with_ref(target, forwards));
        let _ = writeln!(out, "{pad}    {{:ok, handle}} -> {{:ok, {wrapped}}}");
        let _ = writeln!(out, "{pad}    {{:error, error}} -> {{:error, error}}");
        let _ = writeln!(out, "{pad}  end");
    } else {
        let _ = writeln!(out, "{pad}  handle = {}", call_with_ref(target, forwards));
        let _ = writeln!(out, "{pad}  {wrapped}");
    }
}

/// The demand-driven twin of a stream function: `<name>_stream` hands back
/// the running stream instead of an `Enumerable`.
///
/// The `Enumerable` form blocks on `receive` until the next item arrives,
/// which a `GenServer` cannot afford -- its callback owns the process, so
/// every other `handle_call` and `handle_info` waits on the producer. The
/// handle form never blocks: the caller grants demand with
/// `stream_demand/2` and matches the messages it already receives with
/// `stream_message/2`.
pub fn render_stream_handle_fn(
    out: &mut String,
    function: &ir::Function,
    target: &Target<'_>,
    interface: &ir::Interface,
    ns: &str,
    pad: &str,
) {
    let Signature {
        specs,
        params,
        forwards,
    } = signature(function, target, interface, ns);
    let name = format!("{}_stream", target.ex_name);
    let mut lines: Vec<String> = function.docs.clone();
    if !lines.is_empty() {
        lines.push(String::new());
    }
    lines.push(format!(
        "The demand-driven form of `{}/{}`: hands back the running stream",
        target.ex_name,
        params.len()
    ));
    lines.push("instead of an `Enumerable`, so a process grants demand with".to_owned());
    lines.push(format!(
        "`{ns}.stream_demand/2` and matches items in its own"
    ));
    lines.push(format!(
        "`handle_info/2` with `{ns}.stream_message/2` instead of blocking on"
    ));
    lines.push("`receive`.".to_owned());
    doc(out, &lines, pad);
    let handle_spec = format!("{ns}.StreamHandle.t()");
    let ret = function.throws.as_ref().map_or_else(
        || handle_spec.clone(),
        |throws| {
            format!(
                "{{:ok, {handle_spec}}} | {{:error, {ns}.{}.t()}}",
                names::ex_error_name_of(interface, throws)
            )
        },
    );
    let _ = writeln!(out, "{pad}@spec {name}({}) :: {ret}", specs.join(", "));
    let _ = writeln!(out, "{pad}def {name}({}) do", params.join(", "));
    stream_call(
        out,
        function,
        target,
        &forwards,
        pad,
        "%StreamHandle{ref: ref, handle: handle}",
    );
    let _ = writeln!(out, "{pad}end");
}

/// The `<Ns>.StreamHandle` struct: a running stream, addressed by the
/// caller-created reference the producer stamps on every message plus the
/// resource the demand NIF takes.
pub fn stream_handle_module(out: &mut String) {
    out.push_str("\n  defmodule StreamHandle do\n");
    out.push_str("    @moduledoc \"\"\"\n");
    out.push_str("    A running unibind stream, driven by explicit demand.\n\n");
    out.push_str("    Items only arrive after `stream_demand/2` grants credit, one\n");
    out.push_str("    credit per item. The producer sends the owning process\n");
    out.push_str("    `{:unibind_stream, ref, {:item, value}}` per credit and one\n");
    out.push_str("    `{:unibind_stream, ref, :done}` at the end; `stream_message/2`\n");
    out.push_str("    classifies both without the caller matching the wire shape.\n\n");
    out.push_str("    The producer stops when the process that started the stream\n");
    out.push_str("    exits, so a handle is only useful in the process that made it.\n");
    out.push_str("    \"\"\"\n\n");
    out.push_str("    @enforce_keys [:ref, :handle]\n");
    out.push_str("    defstruct [:ref, :handle]\n");
    out.push_str("    @type t :: %__MODULE__{ref: reference(), handle: reference()}\n");
    out.push_str("  end\n");
}

/// The shared stream helpers: demand, message classification, and the
/// private `Enumerable` bridge.
pub fn stream_helper(out: &mut String, ns: &str) {
    out.push_str("\n  @doc \"\"\"\n");
    out.push_str("  Grant `stream`'s producer demand for `n` more items.\n\n");
    out.push_str("  Nothing is produced without demand, so a consumer that never\n");
    out.push_str("  calls this never receives an item.\n");
    out.push_str("  \"\"\"\n");
    let _ = writeln!(
        out,
        "  @spec stream_demand({ns}.StreamHandle.t(), pos_integer()) :: :ok"
    );
    out.push_str("  def stream_demand(%StreamHandle{handle: handle}, n) do\n");
    out.push_str("    Native.unibind_demand(handle, n)\n");
    out.push_str("    :ok\n");
    out.push_str("  end\n");

    out.push_str("\n  @doc \"\"\"\n");
    out.push_str("  Classify `message` against `stream`.\n\n");
    out.push_str("  `:nomatch` for anything that did not come from this stream, so\n");
    out.push_str("  a `handle_info/2` clause can fall through to its own handling.\n");
    out.push_str("  \"\"\"\n");
    let _ = writeln!(
        out,
        "  @spec stream_message({ns}.StreamHandle.t(), term()) ::"
    );
    out.push_str("          {:item, term()} | :done | :nomatch\n");
    out.push_str("  def stream_message(%StreamHandle{ref: ref}, message) do\n");
    out.push_str("    case message do\n");
    out.push_str("      {:unibind_stream, ^ref, {:item, item}} -> {:item, item}\n");
    out.push_str("      {:unibind_stream, ^ref, :done} -> :done\n");
    out.push_str("      _ -> :nomatch\n");
    out.push_str("    end\n");
    out.push_str("  end\n");

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
