defmodule IxMcp.Evaluator do
  @moduledoc """
  Evaluates one cell in the calling process, the way Livebook's
  `Livebook.Runtime.Evaluator` does it: `scan/1` parses to AST, then
  `eval_quoted/3` runs it through `Code.eval_quoted_with_env/4` with
  `:prune_binding` against a snapshot of the workspace context. Output goes
  to whatever group leader the caller installed (each job installs its own
  `IxMcp.Evaluator.IOProxy`).

  `scan/1` is the per-cell gate: code that does not parse is rejected with a
  compiler diagnostic and never evaluated. It is also where the names a cell
  mentions are read off the AST, which is what lets the shared workspace warn
  a cell about somebody else's write before the cell runs on it (#3967).
  Compile-time warnings are collected via `Code.with_diagnostics/2` and
  returned alongside the result.
  """

  @type ok :: {:ok, term(), Code.binding(), Macro.Env.t(), [String.t()]}
  @type failure :: {:runtime_error, String.t(), [String.t()]}

  @typedoc "The names a cell mentions, read off its AST before it runs."
  @type refs :: %{vars: [atom()], modules: [module()]}

  @doc """
  Parse a cell and read off what it mentions: the variables it names and the
  modules it declares. The caller runs the resulting AST with
  `eval_quoted/3`; splitting parse from eval is what lets the workspace warn
  a cell about a variable another cell changed under it *before* the cell
  uses it, since a cell that raises never reaches the merge (#3967).
  """
  @spec scan(String.t()) :: {:ok, Macro.t(), refs()} | {:parse_error, String.t()}
  def scan(code) when is_binary(code) do
    case Code.string_to_quoted(code, file: "cell", columns: true, emit_warnings: false) do
      {:ok, quoted} -> {:ok, quoted, %{vars: variables(quoted), modules: modules(quoted)}}
      {:error, {meta, message, token}} -> {:parse_error, format_parse_error(meta, message, token)}
    end
  end

  # A variable node is `{name, meta, context}` with an atom context; a call
  # carries its arguments there instead, and an alias carries a list. `_`
  # prefixed names are deliberately throwaway and never worth reporting.
  defp variables(quoted) do
    {_quoted, names} =
      Macro.prewalk(quoted, MapSet.new(), fn
        {name, _meta, context} = node, names when is_atom(name) and is_atom(context) ->
          {node, if(reportable_var?(name), do: MapSet.put(names, name), else: names)}

        node, names ->
          {node, names}
      end)

    Enum.sort(names)
  end

  @pseudo_vars [:__MODULE__, :__DIR__, :__ENV__, :__CALLER__, :__STACKTRACE__, :__block__]

  defp reportable_var?(name) do
    name not in @pseudo_vars and not String.starts_with?(Atom.to_string(name), "_")
  end

  defp modules(quoted) do
    {_quoted, declared} =
      Macro.prewalk(quoted, [], fn
        {:defmodule, _meta, [{:__aliases__, _alias_meta, parts} | _rest]} = node, declared ->
          {node, [Module.concat(parts) | declared]}

        node, declared ->
          {node, declared}
      end)

    Enum.reverse(declared)
  end

  @doc "Evaluate an already-parsed cell against a workspace snapshot."
  @spec eval_quoted(Macro.t(), Code.binding(), Macro.Env.t()) :: ok() | failure()
  def eval_quoted(quoted, binding, env) do
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
    "cell:#{line}:#{column}: #{rendered_message}#{heredoc_hint(rendered_message)}"
  end

  # Agents writing cells with embedded source keep opening `"""text"""` on one
  # line, which Elixir forbids; each occurrence costs a full cell retry
  # (#3914). The compiler's two messages for the mistake are stable, so match
  # them exactly instead of rewriting errors broadly.
  @heredoc_hint "\nhint: Elixir heredocs need a newline after the opening triple quote; " <>
                  "for inline strings use ~S(...) or ~s(...)"

  defp heredoc_hint("heredoc allows only whitespace characters" <> _), do: @heredoc_hint

  defp heredoc_hint(message) do
    if message =~ "(for heredoc starting at", do: @heredoc_hint, else: ""
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

  @doc """
  Cut evaluator machinery out of a stacktrace the way Livebook does:
  everything below the compiler's eval frames is plumbing the cell author
  did not write. Used for error stacktraces and for the live stack samples
  the running job stores in the action log (#3546).
  """
  @spec prune_stacktrace(Exception.stacktrace()) :: Exception.stacktrace()
  def prune_stacktrace(stacktrace) do
    stacktrace
    |> Enum.take_while(fn {mod, fun, _arity, _meta} -> {mod, fun} != {:elixir, :eval_forms} end)
    |> Enum.reject(fn {mod, _fun, _arity, _meta} ->
      mod in [IxMcp.Evaluator, Code, :elixir, :erl_eval]
    end)
  end
end
