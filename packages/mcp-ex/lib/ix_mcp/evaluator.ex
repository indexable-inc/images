defmodule IxMcp.Evaluator do
  @moduledoc """
  Evaluates one cell in the calling process, the way Livebook's
  `Livebook.Runtime.Evaluator` does it: parse to AST, then
  `Code.eval_quoted_with_env/4` with `:prune_binding` against a snapshot of
  the workspace context. Output goes to whatever group leader the caller
  installed (each job installs its own `IxMcp.Evaluator.IOProxy`).

  The parse step is the per-cell gate: code that does not parse is rejected
  with a compiler diagnostic and never evaluated. Compile-time warnings are
  collected via `Code.with_diagnostics/2` and returned alongside the result.
  """

  @type ok :: {:ok, term(), Code.binding(), Macro.Env.t(), [String.t()]}
  @type failure :: {:parse_error, String.t()} | {:runtime_error, String.t(), [String.t()]}

  @spec eval(String.t(), Code.binding(), Macro.Env.t()) :: ok() | failure()
  def eval(code, binding, env) when is_binary(code) do
    case Code.string_to_quoted(code, file: env.file, columns: true, emit_warnings: false) do
      {:ok, quoted} ->
        eval_quoted(quoted, binding, env)

      {:error, {meta, message, token}} ->
        {:parse_error, format_parse_error(meta, message, token)}
    end
  end

  defp eval_quoted(quoted, binding, env) do
    {result, diagnostics} =
      Code.with_diagnostics(fn ->
        try do
          {value, binding, env} =
            Code.eval_quoted_with_env(quoted, binding, env, prune_binding: true)

          {:ok, value, binding, env}
        catch
          kind, error ->
            {:error, format_error(kind, error, prune_stacktrace(__STACKTRACE__))}
        end
      end)

    rendered_diags = Enum.map(diagnostics, &format_diagnostic/1)

    case result do
      {:ok, value, binding, env} -> {:ok, value, binding, env, rendered_diags}
      {:error, formatted} -> {:runtime_error, formatted, rendered_diags}
    end
  end

  @doc "Render a value the way the REPL should show it."
  @spec render(term()) :: String.t()
  def render(value) do
    inspect(value, pretty: true, limit: 50, printable_limit: 4096, width: 100)
  end

  defp format_error(kind, error, stacktrace) do
    Exception.format(kind, error, stacktrace)
  end

  defp format_parse_error(meta, message, token) do
    line = Keyword.get(meta, :line, 1)
    column = Keyword.get(meta, :column, 1)
    rendered_message = render_parse_message(message, token)
    "cell:#{line}:#{column}: #{rendered_message}"
  end

  # `Code.string_to_quoted/2` reports the message either as a binary or as a
  # {prefix, suffix} pair the token belongs between.
  defp render_parse_message({prefix, suffix}, token), do: "#{prefix}#{token}#{suffix}"
  defp render_parse_message(message, token), do: "#{message}#{token}"

  defp format_diagnostic(%{severity: severity, message: message, position: position}) do
    "#{severity}: cell:#{format_position(position)}: #{message}"
  end

  defp format_position({line, column}), do: "#{line}:#{column}"
  defp format_position(line) when is_integer(line), do: "#{line}"
  defp format_position(_), do: "?"

  # Everything below the compiler's eval frames is evaluator machinery the
  # cell author did not write; cut it the way Livebook does.
  defp prune_stacktrace(stacktrace) do
    stacktrace
    |> Enum.take_while(fn {mod, fun, _arity, _meta} -> {mod, fun} != {:elixir, :eval_forms} end)
    |> Enum.reject(fn {mod, _fun, _arity, _meta} ->
      mod in [IxMcp.Evaluator, Code, :elixir, :erl_eval]
    end)
  end
end
