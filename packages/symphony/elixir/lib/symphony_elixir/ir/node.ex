defmodule SymphonyElixir.IR.Node do
  @moduledoc """
  One node in the intermediate-representation graph: the durable unit the
  runtime schedules, persists, recovers, and exposes to operators.

  The IR graph is what the DSL interpreter emits. The monadic DSL surface
  composes at the AST layer; evaluating the AST emits these plain-data
  nodes (eval-as-emission). Nothing here is a host closure, so a node can
  be serialized, inspected in the dashboard, retried in isolation, and
  rebuilt deterministically after a restart.

  ## Identity and origin

  - `id` is stable and content-derived (a hash of `ast_origin` plus the
    `expansion_key`), so the same logical node keeps the same id across a
    deterministic replay of the run.
  - `ast_origin` names the AST construct that emitted the node, so a
    retry can re-run the right slice of the interpreter.
  - `expansion_key` distinguishes the instances a dynamic construct emits
    (one per fan-out element, or per `everyNth` iteration); `nil` for
    statically-emitted nodes.

  ## Edges

  `deps` is DERIVED from `inputs`, never hand-written: an input that
  references another node's output is a dependency edge. Two nodes whose
  inputs do not reference each other have no edge and run in parallel.
  This is how the runtime gets auto-parallelism without a `needs:` list.

  ## Kinds

  - `:agent` - an engine turn; carries an `envelope` and a prompt.
  - `:exec` - a shell script under the pack; carries no envelope.
  - `:subrun` - a first-class child run; its output is the child result.
  - `:map_fanout` / `:gate` - dynamic-expansion placeholders that emit
    children when their gating input resolves. They carry no envelope.
  """

  alias SymphonyElixir.Engine.Envelope
  alias SymphonyElixir.IR.Attempt

  @enforce_keys [:id, :ast_origin, :kind, :inputs, :deps, :state]
  defstruct [
    :id,
    :ast_origin,
    :kind,
    :envelope,
    :prompt_ref,
    :inputs,
    :deps,
    :expansion_key,
    :state,
    :output,
    :created_at,
    :updated_at,
    attempts: []
  ]

  @type kind :: :agent | :exec | :subrun | :map_fanout | :gate

  @typedoc """
  Node lifecycle. `:upstream_failed` is set when a dependency failed and
  the node's trigger rule did not allow it to run; `:stranded` is set
  when an attempt's task or BEAM died without a result. Both are distinct
  terminal-ish states so operators can tell why a node did not run.
  """
  @type state ::
          :pending
          | :ready
          | :running
          | :succeeded
          | :failed
          | :skipped
          | :upstream_failed
          | :retrying
          | :cancelled
          | :stranded

  @typedoc """
  A reference an input resolves against. `{:node, id, path}` reads a
  (possibly nested) field of another node's output; `{:literal, value}`
  is a constant computed at expand-time.
  """
  @type input_ref ::
          {:node, String.t(), [term()]}
          | {:literal, term()}

  @typedoc """
  How a node's prompt is built. A `{:skill, ref, bindings}` reference is
  rendered by `SymphonyElixir.Prompt`; `{:inline, text}` is a literal.
  """
  @type prompt_ref ::
          {:skill, String.t(), map()}
          | {:inline, String.t()}
          | nil

  @type t :: %__MODULE__{
          id: String.t(),
          ast_origin: term(),
          kind: kind(),
          envelope: Envelope.t() | nil,
          prompt_ref: prompt_ref(),
          inputs: %{optional(String.t()) => input_ref()},
          deps: [String.t()],
          expansion_key: term() | nil,
          state: state(),
          output: term() | nil,
          attempts: [Attempt.t()],
          created_at: DateTime.t() | nil,
          updated_at: DateTime.t() | nil
        }

  @kinds [:agent, :exec, :subrun, :map_fanout, :gate]
  @states [
    :pending,
    :ready,
    :running,
    :succeeded,
    :failed,
    :skipped,
    :upstream_failed,
    :retrying,
    :cancelled,
    :stranded
  ]
  @terminal [:succeeded, :failed, :skipped, :upstream_failed, :cancelled]

  @doc "The node kinds the interpreter may emit. Source of truth for safe decode."
  @spec kinds() :: [kind()]
  def kinds, do: @kinds

  @doc "Every node state. Source of truth for safe decode."
  @spec states() :: [state()]
  def states, do: @states

  @doc "States after which a node will not run again without operator action."
  @spec terminal_states() :: [state()]
  def terminal_states, do: @terminal

  @spec terminal?(t()) :: boolean()
  def terminal?(%__MODULE__{state: state}), do: state in @terminal

  @doc """
  Build a node with `deps` derived from `inputs`. The caller passes the
  inputs map; the dependency edges fall out of it, so the two can never
  disagree.
  """
  @spec new(keyword()) :: t()
  def new(fields) when is_list(fields) do
    now = DateTime.utc_now()
    inputs = Keyword.get(fields, :inputs, %{})

    %__MODULE__{
      id: Keyword.fetch!(fields, :id),
      ast_origin: Keyword.fetch!(fields, :ast_origin),
      kind: Keyword.fetch!(fields, :kind),
      envelope: Keyword.get(fields, :envelope),
      prompt_ref: Keyword.get(fields, :prompt_ref),
      inputs: inputs,
      deps: deps_from_inputs(inputs),
      expansion_key: Keyword.get(fields, :expansion_key),
      state: Keyword.get(fields, :state, :pending),
      output: Keyword.get(fields, :output),
      attempts: Keyword.get(fields, :attempts, []),
      created_at: now,
      updated_at: now
    }
  end

  @doc "Derive the dependency id list from an inputs map. Pure; the single source of edges."
  @spec deps_from_inputs(%{optional(String.t()) => input_ref()}) :: [String.t()]
  def deps_from_inputs(inputs) when is_map(inputs) do
    inputs
    |> Map.values()
    |> Enum.flat_map(fn
      {:node, id, _path} -> [id]
      {:literal, _} -> []
    end)
    |> Enum.uniq()
  end
end
