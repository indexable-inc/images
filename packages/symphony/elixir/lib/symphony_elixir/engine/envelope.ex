defmodule SymphonyElixir.Engine.Envelope do
  @moduledoc """
  The typed execution envelope for one agent node: which engine runs it,
  with which model, at what reasoning effort, under which permissions, and
  where the engine process lives.

  This replaces the pre-overhaul magic strings, where the engine was
  sniffed from a `codex_model` value (`opus`/`sonnet`/`haiku`/`claude*`),
  `runtime:` meant "where Codex runs" but read like "which engine," and
  codex-only fields (`sandbox`, `approval_policy`) were silently ignored
  for Claude. Every axis is now explicit and validated at load.

  ## Fields

  - `engine` - `:codex`, `:claude`, or `:pi`. Explicit; never inferred
    from the model name.
  - `model` - the model identifier passed through to the engine verbatim
    (for example `gpt-5.3-codex` or `claude-opus-4-8`, or the `opus` /
    `sonnet` / `haiku` aliases Claude accepts). For `:pi` it is a
    pi-harness model alias (`claude`, `codex`, ...): the room-server
    forwards it as `PI_HARNESS_MODEL` and the harness's own model table
    resolves the provider and concrete model.
  - `effort` - reasoning budget. One of `:none`, `:minimal`, `:low`,
    `:medium`, `:high`, `:xhigh`, or `nil` to let the engine pick its
    default.
  - `permissions` - one engine-agnostic level: `:read_only`,
    `:workspace_write`, or `:danger_full_access`. Each engine adapter
    lowers this to its native shape (Codex sandbox + approval policy;
    Claude permission mode / `--dangerously-skip-permissions`).
  - `location` - where the engine process runs: `:local`, `:ixvm`,
    `{:host, name}`, or `{:room, url}`. This is the deployment topology,
    resolved by `Engine.Client` to a concrete room-server URL.

  The dynamic-tool surface (`tools`) is intentionally NOT part of the
  envelope: it is a property of the prompt/skill, not of execution.

  Validation is strict and fails at load (`validate/1`): unknown keys,
  a Claude-looking model under `engine: :codex` (or vice versa), an
  out-of-range effort, an unknown permission level, or a malformed
  location are all errors rather than silently-ignored fields.
  """

  @enforce_keys [:engine, :model]
  defstruct [:engine, :model, :effort, :permissions, :location]

  @engines [:codex, :claude, :pi]
  @efforts [:none, :minimal, :low, :medium, :high, :xhigh]
  @permissions [:read_only, :workspace_write, :danger_full_access]
  # The placement targets a `location` can name. `:host` and `:room` carry
  # a payload (`{:host, name}` / `{:room, url}`) so they are listed as the
  # bare tag the form offers; the operator supplies the payload separately.
  @locations [:local, :ixvm, :host, :room]

  # Derive the flat atom-union types from the same lists the accessors and
  # validator read, so the `@type` (what Dialyzer checks) cannot drift from
  # the runtime vocabulary (what the form and `validate/1` accept). Adding a
  # value to `@engines`/`@efforts`/`@permissions` updates the spec, the
  # accessor, and the check in one place. ENG-1825. `location` is left
  # explicit below because its members carry payloads (`{:host, name}` /
  # `{:room, url}`), a shape the bare-tag `@locations` list cannot express.
  atom_union = fn [first | rest] ->
    Enum.reduce(rest, first, fn value, acc -> {:|, [], [acc, value]} end)
  end

  @type engine :: unquote(atom_union.(@engines))
  @type effort :: unquote(atom_union.(@efforts))
  @type permissions :: unquote(atom_union.(@permissions))
  @type location :: :local | :ixvm | {:host, String.t()} | {:room, String.t()}

  @type t :: %__MODULE__{
          engine: engine(),
          model: String.t(),
          effort: effort() | nil,
          permissions: permissions(),
          location: location()
        }

  @doc "The engines an envelope may select."
  @spec engines() :: [engine()]
  def engines, do: @engines

  @doc "The reasoning-effort values an envelope may declare."
  @spec efforts() :: [effort()]
  def efforts, do: @efforts

  @doc "The permission levels an envelope may declare."
  @spec permission_levels() :: [permissions()]
  def permission_levels, do: @permissions

  @doc "The placement target tags a `location` may name (`:host`/`:room` carry a payload)."
  @spec locations() :: [atom()]
  def locations, do: @locations

  @doc """
  Build a validated envelope from a plain map (string or atom keys), as
  parsed out of a DSL node. Returns `{:ok, envelope}` or `{:error,
  reason}` with a reason that names the offending field.
  """
  @spec from_map(map()) :: {:ok, t()} | {:error, term()}
  def from_map(map) when is_map(map) do
    with {:ok, known} <- reject_unknown_keys(map),
         {:ok, engine} <- fetch_engine(known),
         {:ok, model} <- fetch_model(known),
         {:ok, effort} <- fetch_effort(known),
         {:ok, permissions} <- fetch_permissions(known),
         {:ok, location} <- fetch_location(known) do
      validate(%__MODULE__{
        engine: engine,
        model: model,
        effort: effort,
        permissions: permissions,
        location: location
      })
    end
  end

  def from_map(_other), do: {:error, :envelope_not_map}

  @doc """
  Validate a constructed envelope. The defaulting rule lives here: a
  missing `permissions` defaults to `:workspace_write` and a missing
  `location` defaults to `:local`, both the conservative common case.
  """
  @spec validate(t()) :: {:ok, t()} | {:error, term()}
  def validate(%__MODULE__{} = env) do
    env = %{
      env
      | permissions: env.permissions || :workspace_write,
        location: env.location || :local
    }

    with :ok <- check_engine(env.engine),
         :ok <- check_model(env.model),
         :ok <- check_effort(env.effort),
         :ok <- check_permissions(env.permissions),
         :ok <- check_location(env.location),
         :ok <- check_engine_model_agree(env.engine, env.model) do
      {:ok, env}
    end
  end

  @doc """
  Whether a model string names a Claude model. Used to catch an engine
  and model that disagree. Kept deliberately loose (prefix/alias match)
  rather than an exhaustive list so new model names do not need a code
  change; the check only rejects an unambiguous mismatch.
  """
  @spec claude_model?(String.t()) :: boolean()
  def claude_model?(model) when is_binary(model) do
    normalized = model |> String.trim() |> String.downcase()
    String.starts_with?(normalized, "claude") or normalized in ~w(opus sonnet haiku)
  end

  defp reject_unknown_keys(map) do
    normalized = Map.new(map, fn {k, v} -> {to_string(k), v} end)
    known = ~w(engine model effort permissions location)
    extra = Map.keys(normalized) -- known

    case extra do
      [] -> {:ok, normalized}
      _ -> {:error, {:unknown_envelope_keys, Enum.sort(extra)}}
    end
  end

  defp fetch_engine(%{"engine" => engine}), do: to_known_atom(engine, @engines, :invalid_engine)
  defp fetch_engine(_), do: {:error, {:missing_envelope_field, "engine"}}

  defp fetch_model(%{"model" => model}) when is_binary(model) do
    case String.trim(model) do
      "" -> {:error, {:invalid_model, model}}
      trimmed -> {:ok, trimmed}
    end
  end

  defp fetch_model(%{"model" => other}), do: {:error, {:invalid_model, other}}
  defp fetch_model(_), do: {:error, {:missing_envelope_field, "model"}}

  defp fetch_effort(%{"effort" => nil}), do: {:ok, nil}
  defp fetch_effort(%{"effort" => effort}), do: to_known_atom(effort, @efforts, :invalid_effort)
  defp fetch_effort(_), do: {:ok, nil}

  defp fetch_permissions(%{"permissions" => nil}), do: {:ok, nil}
  defp fetch_permissions(%{"permissions" => perm}), do: to_known_atom(perm, @permissions, :invalid_permissions)
  defp fetch_permissions(_), do: {:ok, nil}

  defp fetch_location(%{"location" => nil}), do: {:ok, nil}
  defp fetch_location(%{"location" => location}), do: parse_location(location)
  defp fetch_location(_), do: {:ok, nil}

  defp parse_location(loc) when loc in [:local, "local"], do: {:ok, :local}
  defp parse_location(loc) when loc in [:ixvm, "ixvm"], do: {:ok, :ixvm}
  defp parse_location(%{"host" => name}) when is_binary(name) and name != "", do: {:ok, {:host, name}}
  defp parse_location(%{"room" => url}) when is_binary(url) and url != "", do: {:ok, {:room, url}}
  defp parse_location({:host, name} = loc) when is_binary(name) and name != "", do: {:ok, loc}
  defp parse_location({:room, url} = loc) when is_binary(url) and url != "", do: {:ok, loc}
  defp parse_location(other), do: {:error, {:invalid_location, other}}

  defp to_known_atom(value, allowed, error) when is_atom(value) do
    if value in allowed, do: {:ok, value}, else: {:error, {error, value}}
  end

  defp to_known_atom(value, allowed, error) when is_binary(value) do
    normalized = value |> String.trim() |> String.downcase()

    case Enum.find(allowed, fn a -> Atom.to_string(a) == normalized end) do
      nil -> {:error, {error, value}}
      atom -> {:ok, atom}
    end
  end

  defp to_known_atom(value, _allowed, error), do: {:error, {error, value}}

  defp check_engine(engine) when engine in @engines, do: :ok
  defp check_engine(other), do: {:error, {:invalid_engine, other}}

  defp check_model(model) when is_binary(model) and model != "", do: :ok
  defp check_model(other), do: {:error, {:invalid_model, other}}

  defp check_effort(nil), do: :ok
  defp check_effort(effort) when effort in @efforts, do: :ok
  defp check_effort(other), do: {:error, {:invalid_effort, other}}

  defp check_permissions(perm) when perm in @permissions, do: :ok
  defp check_permissions(other), do: {:error, {:invalid_permissions, other}}

  defp check_location(:local), do: :ok
  defp check_location(:ixvm), do: :ok
  defp check_location({:host, name}) when is_binary(name) and name != "", do: :ok
  defp check_location({:room, url}) when is_binary(url) and url != "", do: :ok
  defp check_location(other), do: {:error, {:invalid_location, other}}

  # The mismatch guard the pre-overhaul code could not express: a Claude
  # model under engine: :codex (or a non-Claude model under :claude) is a
  # load error, not a silent mis-route.
  defp check_engine_model_agree(:codex, model) do
    if claude_model?(model), do: {:error, {:engine_model_mismatch, :codex, model}}, else: :ok
  end

  defp check_engine_model_agree(:claude, model) do
    if claude_model?(model), do: :ok, else: {:error, {:engine_model_mismatch, :claude, model}}
  end

  # Pi is a meta-harness fronting multiple providers: its model value is a
  # pi-harness alias ("claude", "codex", ...) resolved by the harness's own
  # model table, so a Claude-looking model under :pi is correct, not a
  # mismatch. Existence of the alias is validated by the harness at turn
  # start, where the table lives.
  defp check_engine_model_agree(:pi, _model), do: :ok
end
