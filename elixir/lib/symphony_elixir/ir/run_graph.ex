defmodule SymphonyElixir.IR.RunGraph do
  @moduledoc """
  The durable state of one DSL workflow run: the reified AST it started
  from, the IR nodes materialized so far, and the append-only expansion
  log that makes a restart deterministic.

  ## Why the expansion log exists

  Dynamic constructs (`when`, `everyNth`, fan-out) expand the graph at
  runtime based on data that arrived from engines, so the materialized
  graph is not a pure function of the source alone. Each expansion is
  recorded as an event ("gate G saw output X, emitted nodes [...]").

  On restart the runtime does not restore a live computation. It loads
  this record, replays `expansion_log` against `ast` to rebuild the exact
  same materialized graph, reconciles any node left `:running`, recomputes
  the ready set, and resumes. The interpreter is re-run, never resurrected
  from a frozen closure. The invariant the runtime tests assert is
  `replay(ast, expansion_log) == nodes`.

  `source_hash` snapshots the `.sym` source the run started with, the same
  way the pre-overhaul runtime snapshotted the DAG, so editing the pack
  does not perturb runs in flight.
  """

  alias SymphonyElixir.IR.Node

  @enforce_keys [:run_id, :source_hash, :status, :nodes]
  defstruct [
    :run_id,
    :source_hash,
    :ast,
    :trigger,
    :status,
    :placement,
    :created_at,
    :updated_at,
    nodes: %{},
    expansion_log: [],
    audit_log: []
  ]

  @type status :: :pending | :running | :succeeded | :failed | :cancelled

  @statuses [:pending, :running, :succeeded, :failed, :cancelled]

  @doc "The run statuses a persisted graph may hold. Source of truth for safe decode."
  @spec statuses() :: [status()]
  def statuses, do: @statuses

  @typedoc """
  One dynamic-expansion event. `origin` is the AST construct that
  expanded; `observed` is the gating output it reacted to; `emitted` is
  the list of node ids it added. Replaying the log in order reconstructs
  the materialized graph.
  """
  @type expansion_event :: %{
          origin: term(),
          observed: term(),
          emitted: [String.t()],
          at: DateTime.t()
        }

  @typedoc """
  One operator action recorded for audit. `action` is the operation
  (`:cancel`, `:retry_node`, `:rerun`, `:clear_failed`); `target` is the
  node id it acted on or `nil` for run-wide actions; `actor` identifies
  who requested it (an operator id, or `:system` for automatic actions);
  `detail` carries action-specific context. The log is append-only and
  ordered, so it reconstructs the operator history of a run.
  """
  @type audit_event :: %{
          action: atom(),
          target: String.t() | nil,
          actor: term(),
          detail: term(),
          at: DateTime.t()
        }

  @type t :: %__MODULE__{
          run_id: String.t(),
          source_hash: binary(),
          ast: term() | nil,
          trigger: map() | nil,
          status: status(),
          placement: %{declared: term(), effective: :ixvm | :host | :local | nil} | nil,
          nodes: %{String.t() => Node.t()},
          expansion_log: [expansion_event()],
          audit_log: [audit_event()],
          created_at: DateTime.t() | nil,
          updated_at: DateTime.t() | nil
        }

  @spec new(String.t(), binary(), term()) :: t()
  def new(run_id, source_hash, ast) when is_binary(run_id) and is_binary(source_hash) do
    now = DateTime.utc_now()

    %__MODULE__{
      run_id: run_id,
      source_hash: source_hash,
      ast: ast,
      status: :pending,
      nodes: %{},
      expansion_log: [],
      audit_log: [],
      created_at: now,
      updated_at: now
    }
  end

  @doc "Add or replace nodes, keeping the map keyed by node id."
  @spec put_nodes(t(), [Node.t()]) :: t()
  def put_nodes(%__MODULE__{nodes: nodes} = graph, new_nodes) when is_list(new_nodes) do
    merged = Enum.reduce(new_nodes, nodes, fn %Node{id: id} = n, acc -> Map.put(acc, id, n) end)
    %{graph | nodes: merged, updated_at: DateTime.utc_now()}
  end

  @doc "Append a dynamic-expansion event. The order is load-bearing for replay."
  @spec append_expansion(t(), term(), term(), [String.t()]) :: t()
  def append_expansion(%__MODULE__{expansion_log: log} = graph, origin, observed, emitted) do
    event = %{origin: origin, observed: observed, emitted: emitted, at: DateTime.utc_now()}
    %{graph | expansion_log: log ++ [event], updated_at: DateTime.utc_now()}
  end

  @doc """
  Append an operator audit event. Append-only and ordered, so the log is
  the durable record of who acted on the run and how. `target` is the node
  id for a node-scoped action or `nil` for a run-wide one.
  """
  @spec append_audit(t(), atom(), String.t() | nil, term(), term()) :: t()
  def append_audit(%__MODULE__{audit_log: log} = graph, action, target, actor, detail)
      when is_atom(action) do
    event = %{action: action, target: target, actor: actor, detail: detail, at: DateTime.utc_now()}
    %{graph | audit_log: log ++ [event], updated_at: DateTime.utc_now()}
  end
end
