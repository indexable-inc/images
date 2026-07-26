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
  Parse a cell and read off what it mentions: the workspace variables it
  reads and the modules it declares. The caller runs the resulting AST with
  `eval_quoted/3`; splitting parse from eval is what lets the workspace warn
  a cell about a variable another cell changed under it *before* the cell
  uses it, since a cell that raises never reaches the merge (#3967).
  """
  @spec scan(String.t()) :: {:ok, Macro.t(), refs()} | {:parse_error, String.t()}
  def scan(code) when is_binary(code) do
    # `file:` matches what `IxMcp.Workspace` puts in the eval env, which is
    # what error positions are reported against.
    case Code.string_to_quoted(code, file: "cell", columns: true, emit_warnings: false) do
      {:ok, quoted} -> {:ok, quoted, read_refs(quoted)}
      {:error, {meta, message, token}} -> {:parse_error, format_parse_error(meta, message, token)}
    end
  end

  # One walk, three questions: which names does the cell mention, which of
  # those does it introduce itself, and which modules does it declare. A name
  # the cell introduces in a clause head is that clause's own variable and
  # says nothing about the workspace's, so `fn body -> body end` must not
  # count as reading the workspace's `body`. An `=` is deliberately not a
  # binder here: `body = String.trim(body)` genuinely reads the old value.
  defp read_refs(quoted) do
    {_quoted, acc} =
      Macro.prewalk(quoted, %{vars: MapSet.new(), bound: MapSet.new(), modules: []}, &visit/2)

    %{
      vars: acc.vars |> MapSet.difference(acc.bound) |> Enum.sort(),
      modules: Enum.reverse(acc.modules)
    }
  end

  # A `defmodule`/`def` body is its own scope and cannot see cell variables at
  # all, and a `quote` block is code that may never run; neither tells us
  # anything about what this cell reads, so both are walked for nothing but
  # the module names `defmodule` claims.
  defp visit({:quote, _meta, _args} = node, acc), do: {skip(node), acc}

  defp visit({:defmodule, _meta, [alias_node | rest]} = node, acc) do
    acc =
      case declared_module(alias_node) do
        nil -> acc
        module -> %{acc | modules: [module | acc.modules]}
      end

    # Walk the body for nested defmodules only; nothing in it reads the cell.
    {_walked, inner} =
      Macro.prewalk(rest, %{acc | vars: MapSet.new(), bound: MapSet.new()}, &visit/2)

    {skip(node), %{acc | modules: inner.modules}}
  end

  defp visit({def_kind, _meta, _args} = node, acc) when def_kind in [:def, :defp, :defmacro],
    do: {skip(node), acc}

  # `fn`, `case`, `with`, `for`, `try` and friends all reach the compiler as
  # `->` clauses; a `<-` generator binds the same way.
  defp visit({:->, _meta, [head, _body]} = node, acc),
    do: {node, %{acc | bound: MapSet.union(acc.bound, pattern_vars(head))}}

  defp visit({:<-, _meta, [pattern, _source]} = node, acc),
    do: {node, %{acc | bound: MapSet.union(acc.bound, pattern_vars(pattern))}}

  # A pin reads the outer value even inside a pattern, so it is a mention
  # that no clause head can take away.
  defp visit({:^, _meta, [{name, _var_meta, context}]} = node, acc)
       when is_atom(name) and is_atom(context) do
    {node, %{acc | bound: MapSet.delete(acc.bound, name), vars: MapSet.put(acc.vars, name)}}
  end

  defp visit({name, _meta, context} = node, acc) when is_atom(name) and is_atom(context) do
    {node, if(mentionable?(name), do: %{acc | vars: MapSet.put(acc.vars, name)}, else: acc)}
  end

  defp visit(node, acc), do: {node, acc}

  # Replacing a node with an atom is how a prewalk declines to descend.
  defp skip(_node), do: :__skipped__

  # `defmodule __MODULE__.Inner` and `defmodule unquote(name)` name a module
  # the AST cannot resolve; there is nothing to claim and `Module.concat/1`
  # would raise on the attempt.
  defp declared_module({:__aliases__, _meta, parts}) do
    if Enum.all?(parts, &is_atom/1), do: Module.concat(parts)
  end

  defp declared_module(_other), do: nil

  defp pattern_vars(pattern) do
    {_pattern, names} =
      Macro.prewalk(pattern, MapSet.new(), fn
        {:^, _meta, _args} = node, names ->
          {skip(node), names}

        {name, _meta, context} = node, names when is_atom(name) and is_atom(context) ->
          {node, if(mentionable?(name), do: MapSet.put(names, name), else: names)}

        node, names ->
          {node, names}
      end)

    names
  end

  @pseudo_vars [:__MODULE__, :__DIR__, :__ENV__, :__CALLER__, :__STACKTRACE__, :__block__]

  defp mentionable?(name) do
    name not in @pseudo_vars and not String.starts_with?(Atom.to_string(name), "_")
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
