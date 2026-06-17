defmodule SymphonyElixir.Prompt do
  @moduledoc """
  Render an agent node's prompt from its `prompt_ref` and the bindings the
  interpreter resolved for it. This is the piece the engine client needs
  to turn a `{:skill, name, bindings}` reference into the actual text an
  engine runs.

  Two prompt shapes flow out of the DSL (`SymphonyElixir.DSL.AST`):

  - `{:inline, text}` is literal text, already interpolated at expand
    time. `build/2` returns it verbatim.
  - `{:skill, name, bindings}` names a markdown skill body under the
    active pack. `build/2` loads the body through the injected resolver,
    expands shared `{{partial:name}}` includes, and interpolates
    `${binding}` and `${binding.path}` placeholders from `bindings`.

  ## Why a resolver is injected

  The body source (which pack, which directory, hot-reload vs a snapshot)
  is owned by the catalog layer, not by prompt rendering. `build/2` takes
  a `:resolver` function `name -> {:ok, body} | {:error, reason}` so this
  module stays pack-agnostic and unit-testable without touching the
  filesystem. `SymphonyElixir.Prompt.Skill` is the default resolver over a
  pack's `skills/` directory.

  ## Interpolation

  A placeholder is `${path}` where `path` is a dotted key sequence into
  the bindings map (`${ticket.id}` reads `bindings["ticket"]["id"]`). A
  placeholder whose binding is missing is a render error rather than a
  silently empty substitution, so a skill that references an input the
  node never bound fails loudly. Literal `$` that is not a placeholder is
  left untouched: only `${...}` is special.

  Write `$${path}` to emit a literal `${path}` with no binding lookup.
  Skill bodies routinely embed shell, Make, or JS-template snippets whose
  own `${VAR}` would otherwise be read as a binding and fail the run; the
  doubled `$$` is the escape that lets those survive. The escape consumes
  one `$`, so `$${x}` renders `${x}`.
  """

  @typedoc "Resolver from a skill name to its raw markdown body."
  @type resolver :: (String.t() -> {:ok, String.t()} | {:error, term()})

  @typedoc "Bindings the interpreter resolved for the prompt (string keys, literal values)."
  @type bindings :: %{optional(String.t()) => term()}

  # Group 1 is an optional leading `$` that escapes the match to a literal
  # `${...}`; group 2 is the binding path. Both forms match in one pass so
  # an escaped `$${x}` cannot also match as the placeholder `${x}` at the
  # next offset.
  @placeholder ~r/(\$?)\$\{([A-Za-z0-9_.]+)\}/

  @doc """
  Build the prompt text for a `prompt_ref`. `opts` carries the skill body
  `:resolver` (required for a `{:skill, _, _}` ref) and an optional
  `:partial_resolver` for `{{partial:name}}` includes.
  """
  @spec build(SymphonyElixir.IR.Node.prompt_ref(), keyword()) :: {:ok, String.t()} | {:error, term()}
  def build(prompt_ref, opts \\ [])

  # astlog-ignore: public-def-needs-spec
  def build({:inline, text}, _opts) when is_binary(text), do: {:ok, text}
  # astlog-ignore: public-def-needs-spec
  def build({:inline, nil}, _opts), do: {:error, :unresolved_inline_prompt}

  # astlog-ignore: public-def-needs-spec
  def build({:skill, name, bindings}, opts) when is_binary(name) and is_map(bindings) do
    with {:ok, resolver} <- fetch_resolver(opts),
         {:ok, body} <- resolver.(name),
         {:ok, expanded} <- expand_partials(body, opts) do
      render(expanded, bindings)
    end
  end

  # astlog-ignore: public-def-needs-spec
  def build(nil, _opts), do: {:error, :missing_prompt_ref}
  # astlog-ignore: public-def-needs-spec
  def build(other, _opts), do: {:error, {:invalid_prompt_ref, other}}

  @doc """
  Interpolate `${path}` placeholders in `body` from `bindings`. Pure: the
  core of `build/2`, exposed so a caller can render an already-loaded body
  and so tests can assert interpolation without a resolver. A placeholder
  with no matching binding returns `{:error, {:unbound_placeholder, path}}`.
  An escaped `$${path}` collapses to a literal `${path}` and is never
  looked up, so a missing binding there is not an error.
  """
  @spec render(String.t(), bindings()) :: {:ok, String.t()} | {:error, term()}
  def render(body, bindings) when is_binary(body) and is_map(bindings) do
    # Only the unescaped matches (empty group 1) are real placeholders, so
    # an escaped `$${x}` neither needs a binding nor reports one missing.
    missing =
      @placeholder
      |> Regex.scan(body, capture: :all_but_first)
      |> Enum.filter(fn [escape, _path] -> escape == "" end)
      |> Enum.map(fn [_escape, path] -> path end)
      |> Enum.uniq()
      |> Enum.find(fn path -> resolve_binding(bindings, path) == :missing end)

    case missing do
      nil ->
        rendered =
          Regex.replace(@placeholder, body, fn
            _full, "", path -> to_text(fetch_binding(bindings, path))
            _full, _escape, path -> "${" <> path <> "}"
          end)

        {:ok, rendered}

      path ->
        {:error, {:unbound_placeholder, path}}
    end
  end

  defp fetch_resolver(opts) do
    case Keyword.get(opts, :resolver) do
      fun when is_function(fun, 1) -> {:ok, fun}
      _ -> {:error, :missing_skill_resolver}
    end
  end

  # `{{partial:name}}` includes reuse the catalog's partial convention.
  # When no partial resolver is supplied, a body with no partial token
  # passes through; a body that references a partial without a resolver is
  # a render error so a half-rendered prompt never reaches an engine.
  @partial ~r/\{\{partial:([A-Za-z0-9_-]+)\}\}/

  defp expand_partials(body, opts) do
    names = @partial |> Regex.scan(body, capture: :all_but_first) |> List.flatten() |> Enum.uniq()

    if names == [] do
      {:ok, body}
    else
      case Keyword.get(opts, :partial_resolver) do
        fun when is_function(fun, 1) -> substitute_partials(body, names, fun)
        _ -> {:error, {:missing_partial_resolver, names}}
      end
    end
  end

  defp substitute_partials(body, names, resolver) do
    case load_partials(names, resolver) do
      {:ok, map} -> {:ok, Regex.replace(@partial, body, fn _full, name -> Map.fetch!(map, name) end)}
      {:error, _} = err -> err
    end
  end

  defp load_partials(names, resolver) do
    Enum.reduce_while(names, {:ok, %{}}, fn name, {:ok, acc} ->
      case resolver.(name) do
        {:ok, body} when is_binary(body) -> {:cont, {:ok, Map.put(acc, name, body)}}
        {:error, reason} -> {:halt, {:error, {:missing_partial, name, reason}}}
      end
    end)
  end

  defp resolve_binding(bindings, path) do
    case fetch_binding(bindings, path) do
      :missing -> :missing
      _value -> :present
    end
  end

  defp fetch_binding(bindings, path) do
    keys = String.split(path, ".")
    dig(bindings, keys)
  end

  defp dig(value, []), do: value

  defp dig(value, [key | rest]) when is_map(value) do
    case Map.fetch(value, key) do
      {:ok, inner} -> dig(inner, rest)
      :error -> :missing
    end
  end

  defp dig(_value, _keys), do: :missing

  defp to_text(value) when is_binary(value), do: value
  defp to_text(value), do: to_string(value)
end
