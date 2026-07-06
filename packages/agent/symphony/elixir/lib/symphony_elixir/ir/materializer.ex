defmodule SymphonyElixir.IR.Materializer do
  @moduledoc """
  The seam between the DSL interpreter and the durable IR graph. It runs
  `DSL.Interpreter.expand/3` and folds the result into a `RunGraph`: the
  initial materialization at run start, and the dynamic re-expansion that
  emits a gate's or fan-out's children once the gating output arrives.

  Pure: every function takes a graph (or the pieces to build one) and
  returns a new graph. The `Runtime` GenServer owns scheduling and
  persistence; this module owns turning interpreter output into graph
  nodes and expansion-log events.

  ## Why re-expansion is a merge, not a replace

  `DSL.Interpreter.expand/3` is a pure function of `(ast, known_outputs,
  expansion_log)`. Each call re-emits every node the current
  `known_outputs` justify, with content-derived stable ids. So the
  materializer cannot blindly overwrite: a node already materialized and
  possibly already `:running` or `:succeeded` must keep its live state. It
  merges by id, adding only nodes the graph has never seen and preserving
  the state of nodes it already has. The expansion log grows the same way:
  the interpreter returns the prior log plus this pass's new events, so
  the materializer takes the returned log as the new one.

  This is exactly the restart-replay invariant from `IR.RunGraph`:
  re-running the interpreter against the recorded outputs and log
  reconstructs the same node set, so a live re-expansion and a
  cold replay produce identical graphs.
  """

  alias SymphonyElixir.DSL.AST
  alias SymphonyElixir.DSL.Interpreter
  alias SymphonyElixir.Engine.Envelope
  alias SymphonyElixir.IR.Node
  alias SymphonyElixir.IR.RunGraph

  @doc """
  Build the initial `RunGraph` for a run from its AST. Expands against no
  known outputs (the static slice of the graph plus any gate/fan-out
  placeholders), validates every agent node's envelope, and records the
  expansion log the first pass produced.

  Envelope validation is the load-time fail-fast the overhaul plan
  requires: the interpreter emits each agent's envelope as a raw spec map,
  and this is the boundary that lowers it to a typed `Engine.Envelope` (or
  fails the whole run with `{:error, {:invalid_envelope, node_id, reason}}`
  rather than scheduling a node with a malformed envelope).
  """
  @spec materialize(String.t(), binary(), AST.workflow()) :: {:ok, RunGraph.t()} | {:error, term()}
  def materialize(run_id, source_hash, ast) when is_binary(run_id) and is_binary(source_hash) do
    {nodes, pending, log} = Interpreter.expand(ast, %{}, [])
    nodes = thread_pending_deps(nodes, pending)

    with {:ok, validated} <- validate_envelopes(nodes) do
      graph =
        run_id
        |> RunGraph.new(source_hash, ast)
        |> RunGraph.put_nodes(validated)
        |> put_log(log)
        |> Map.put(:status, :running)

      {:ok, graph}
    end
  end

  @doc """
  Re-expand the AST against the outputs of succeeded nodes and merge the
  fresh expansion into the graph. Returns `{graph, new_node_ids}` so the
  runtime knows which ids first appeared on this pass.

  The merge is by id, with state deciding what survives:

  - A node the graph has never seen is added.
  - A node the graph holds at `:pending` is replaced by the fresh
    version, because the only thing that changes a still-pending node is
    a dependency output it was waiting on resolving. This is what lets a
    deferred prompt fold from `{:inline, nil}` to its real text, and a
    skill binding fold from an unresolved node ref to the resolved value,
    once the referenced node succeeds. The node's `created_at` and
    `attempts` are preserved so the identity and history are unbroken.
  - A node the graph holds `:running` or terminal keeps its live state;
    the fresh expansion never clobbers a node mid-flight.

  A resolved gate retires its placeholder. A `when`/`map` placeholder the
  interpreter emitted while its gating output was unknown is no longer
  re-emitted once that output resolves (the interpreter emits the body
  instead). A still-`:pending` placeholder that the fresh expansion no
  longer produces is marked `:skipped`, so the placeholder does not sit
  `:pending` forever and deadlock the run. This is the load-bearing pair
  with the runtime's deadlock guard.

  A newly-emitted agent node has its envelope validated and lowered the
  same way as the initial pass; an invalid envelope on a dynamically
  emitted child fails with `{:error, {:invalid_envelope, id, reason}}`.

  A graph with no `ast` (a hand-built graph in a test, or a pre-DSL run)
  is returned unchanged with no new ids: there is nothing to re-expand.
  """
  @spec expand_dynamic(RunGraph.t()) :: {:ok, RunGraph.t(), [String.t()]} | {:error, term()}
  def expand_dynamic(%RunGraph{ast: %{kind: :workflow} = ast} = graph) do
    known = known_outputs(graph)
    {nodes, pending, log} = Interpreter.expand(ast, known, graph.expansion_log)
    nodes = thread_pending_deps(nodes, pending)

    {to_apply, new_ids} = mergeable(nodes, graph.nodes)
    emitted_ids = MapSet.new(nodes, & &1.id)

    with {:ok, validated} <- validate_envelopes(to_apply) do
      updated =
        graph
        |> RunGraph.put_nodes(validated)
        |> retire_resolved_placeholders(emitted_ids)
        |> put_log(log)

      {:ok, updated, new_ids}
    end
  end

  # A graph with no AST, or an `ast` that is not a reified workflow (a
  # hand-built graph in a test, or a pre-DSL run), has nothing to
  # re-expand. Return it unchanged.
  def expand_dynamic(%RunGraph{} = graph), do: {:ok, graph, []}

  @placeholder_kinds [:gate, :map_fanout]

  # A placeholder that the fresh expansion no longer emits has resolved: it
  # waited on an output that is now known, so the interpreter produced the
  # body in its place. Mark the leftover placeholder `:skipped` so it
  # leaves the schedulable/non-terminal set. Only `:pending` placeholders
  # are retired; one already terminal or running is left alone.
  defp retire_resolved_placeholders(%RunGraph{nodes: nodes} = graph, emitted_ids) do
    retired =
      Map.new(nodes, fn {id, node} ->
        if node.kind in @placeholder_kinds and node.state == :pending and not MapSet.member?(emitted_ids, id) do
          {id, %{node | state: :skipped, updated_at: DateTime.utc_now()}}
        else
          {id, node}
        end
      end)

    %{graph | nodes: retired, updated_at: DateTime.utc_now()}
  end

  @doc """
  The outputs of every succeeded node, keyed by node id. This is the
  `known_outputs` the interpreter folds into pure values and gate
  decisions, so a gate sees a dependency's result exactly once that
  dependency reaches `:succeeded`.
  """
  @spec known_outputs(RunGraph.t()) :: %{optional(String.t()) => term()}
  def known_outputs(%RunGraph{nodes: nodes}) do
    for {id, %Node{state: :succeeded, output: output}} <- nodes, into: %{}, do: {id, output}
  end

  # Lower each agent node's raw envelope spec map to a typed, validated
  # `Engine.Envelope`. A non-agent node has no envelope and passes
  # through. An already-lowered envelope (a struct) is left as is, so this
  # is idempotent across re-expansions. The first invalid envelope fails
  # the whole pass with the offending node id.
  defp validate_envelopes(nodes) do
    nodes
    |> Enum.reduce_while({:ok, []}, fn node, {:ok, acc} ->
      case lower_envelope(node) do
        {:ok, lowered} -> {:cont, {:ok, [lowered | acc]}}
        {:error, _} = err -> {:halt, err}
      end
    end)
    |> case do
      {:ok, lowered} -> {:ok, Enum.reverse(lowered)}
      {:error, _} = err -> err
    end
  end

  defp lower_envelope(%Node{kind: :agent, envelope: %Envelope{}} = node), do: {:ok, node}

  defp lower_envelope(%Node{kind: :agent, envelope: spec, id: id} = node) when is_map(spec) do
    case Envelope.from_map(spec) do
      {:ok, envelope} -> {:ok, %{node | envelope: envelope}}
      {:error, reason} -> {:error, {:invalid_envelope, id, reason}}
    end
  end

  defp lower_envelope(%Node{kind: :agent, id: id}), do: {:error, {:missing_envelope, id}}
  defp lower_envelope(%Node{} = node), do: {:ok, node}

  # Partition the fresh expansion against the graph's nodes into the set to
  # apply. A re-expansion re-emits every already-materialized node with its
  # stable id, so a blind overwrite would clobber a node mid-flight. The
  # rule is by state: a brand-new id is added; an existing `:pending` node
  # is replaced (this is how a deferred prompt or skill binding folds to
  # its resolved value once the awaited output arrives), carrying its
  # `created_at` and `attempts` so identity and history survive; an
  # existing `:running` or terminal node is left untouched. `new_ids` are
  # the ids that first appeared this pass.
  defp mergeable(emitted, existing) do
    {to_apply, new_ids} =
      Enum.reduce(emitted, {[], []}, fn %Node{id: id} = node, {apply_acc, new_ids} ->
        case Map.fetch(existing, id) do
          :error ->
            {[node | apply_acc], [id | new_ids]}

          {:ok, %Node{state: :pending} = old} ->
            # Union the deps: once a deferred prompt or binding folds, its
            # input ref becomes a literal and the fresh pass derives no edge
            # for it, but the dependency is part of the run's true structure
            # and the operator view should keep it. The edge points at an
            # already-succeeded node, so deps stay satisfied and scheduling
            # is unaffected.
            merged = %{
              node
              | created_at: old.created_at,
                attempts: old.attempts,
                deps: Enum.uniq(old.deps ++ node.deps)
            }

            {[merged | apply_acc], new_ids}

          {:ok, _live} ->
            {apply_acc, new_ids}
        end
      end)

    {Enum.reverse(to_apply), Enum.reverse(new_ids)}
  end

  # Fold the interpreter's `pending` set into node deps. An effect whose
  # prompt or input mixes a literal with a node read cannot be one
  # `input_ref`, so the interpreter reports the awaited node ids in
  # `pending` rather than as input edges. Without this the node would carry
  # no dep for that read, become ready immediately, and run before the
  # output it interpolates exists. Matching `{:awaiting, origin, needed}`
  # to a node by id covers the statically-emitted effects (`node.id` equals
  # the AST origin when `expansion_key` is nil); fan-out children resolve
  # their edges through keyed inputs already.
  defp thread_pending_deps(nodes, pending) do
    awaited =
      Enum.reduce(pending, %{}, fn {:awaiting, origin, needed}, acc ->
        Map.update(acc, origin, needed, &(&1 ++ needed))
      end)

    Enum.map(nodes, fn %Node{} = node ->
      case Map.get(awaited, node.id, []) do
        [] -> node
        extra -> %{node | deps: Enum.uniq(node.deps ++ extra)}
      end
    end)
  end

  # The interpreter returns the full log (prior + this pass). Adopt it as
  # the graph's log directly, normalizing each event to the RunGraph
  # expansion_event shape (the interpreter's events carry no `at`).
  defp put_log(%RunGraph{} = graph, log) do
    normalized = Enum.map(log, &normalize_event/1)
    %{graph | expansion_log: normalized, updated_at: DateTime.utc_now()}
  end

  defp normalize_event(%{at: _} = event), do: event
  defp normalize_event(event), do: Map.put(event, :at, DateTime.utc_now())
end
