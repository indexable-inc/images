defmodule SymphonyElixir.Runtime.Recovery do
  @moduledoc """
  Restart reconciliation for the IR runtime, the correctness core of
  issue #90.

  When the BEAM restarts, the live runtime processes and their monitored
  tasks are gone, but the `RunGraph` persisted by `IR.Store` survives. Two
  facts must be rebuilt from that record:

  1. The materialized graph. The graph is not a pure function of the
     source: dynamic constructs (`when`, `everyNth`, fan-out) expanded it
     based on data that arrived at runtime. Each expansion was recorded in
     the append-only `expansion_log`. `replay/2` re-runs the interpreter's
     expansion against the AST in log order, reproducing the identical
     node set. The invariant the tests assert is
     `replay(ast, log) == live graph`.

  2. The orphaned `:running` nodes. A node persisted `:running` had a live
     task that the restart killed. Its owning task is provably gone, so
     the runtime cannot assume the attempt succeeded or even that it had
     no side effects. `reconcile/2` resolves each such node by policy.

  ## The non-idempotent retry safety rule

  Agent turns are not idempotent: a turn may have pushed a commit before
  the BEAM died. Blindly auto-retrying a stranded agent node could push a
  second commit, double-open a PR, or repeat any other side effect. So the
  default policy is conservative and matches the plan's locked decision:

  - Retry is opt-in per node (`node.inputs["__retry__"]` carries
    `{:literal, true}`). A node that did not opt in is never auto-retried.
  - Even an opt-in node is only auto-retried when its attempt had no
    observed side effect. An attempt that recorded a `thread_id` is
    treated as having possibly acted, so it routes to a human-review
    `:stranded` state instead of a blind retry.

  Recovery first tries to reattach: if `EngineClient.status/1` reports the
  thread `:running` it is left `:running` for the live runtime to keep
  monitoring (a future workstream re-subscribes); if it reports
  `{:finished, result}` the result is harvested through `IR.Graph`. Only an
  `:unknown` thread falls through to the strand/retry policy.
  """

  alias SymphonyElixir.IR.Attempt
  alias SymphonyElixir.IR.Graph
  alias SymphonyElixir.IR.Node
  alias SymphonyElixir.IR.RunGraph
  alias SymphonyElixir.IR.Store

  @doc """
  Rebuild a materialized graph from an AST and an expansion log by
  replaying each expansion in order through `expand_fun`. The function
  signature mirrors the interpreter's `expand`: given the AST, the
  observed gating output, and the running node map, it returns the nodes
  that expansion emits. Replaying the same log against the same AST is
  deterministic, so two replays produce the same node set.

  `expand_fun` defaults to a stub that emits nothing, which is correct for
  a statically-materialized graph (no dynamic expansion). WS-5 wires the
  real interpreter in.
  """
  @spec replay(RunGraph.t(), (term(), term(), %{String.t() => Node.t()} -> [Node.t()])) ::
          RunGraph.t()
  def replay(%RunGraph{} = graph, expand_fun \\ &default_expand/3) when is_function(expand_fun, 3) do
    Enum.reduce(graph.expansion_log, graph, fn event, acc ->
      emitted = expand_fun.(event.origin, event.observed, acc.nodes)
      RunGraph.put_nodes(acc, emitted)
    end)
  end

  defp default_expand(_origin, _observed, _nodes), do: []

  @doc """
  Reconcile a reloaded graph after a restart. For every node left
  `:running`, probe the engine and apply policy, then recompute nothing
  else: the ready set falls out of `IR.Graph.ready_nodes/1` on the
  returned graph. Pure except for the `status_fun` probe, which a test
  supplies as a fake.

  `status_fun` is `EngineClient.status/1` (probe by `thread_id`). The
  returned graph has no `:running` nodes; each has moved to `:succeeded`,
  `:failed`, `:retrying`, or `:stranded` by the rules in the moduledoc.
  """
  @spec reconcile(RunGraph.t(), (String.t() | nil -> term()), keyword()) :: RunGraph.t()
  def reconcile(%RunGraph{} = graph, status_fun, store_opts \\ []) when is_function(status_fun, 1) do
    graph
    |> Graph.running_nodes()
    |> Enum.reduce(graph, fn node, acc -> reconcile_node(acc, node, status_fun, store_opts) end)
  end

  defp reconcile_node(%RunGraph{} = graph, %Node{} = node, status_fun, store_opts) do
    case reconcile_subrun(graph, node, store_opts) do
      {:ok, reconciled} -> reconciled
      :not_subrun -> reconcile_engine_node(graph, node, status_fun)
    end
  end

  defp reconcile_engine_node(%RunGraph{} = graph, %Node{} = node, status_fun) do
    case status_fun.(current_thread_id(node)) do
      :running ->
        # The engine still owns the turn. Leave it :running; the live
        # runtime keeps monitoring (re-subscription lands in a later
        # workstream). Recovery does not strand a turn the engine can
        # still account for.
        graph

      {:finished, result} ->
        graph
        |> mark_attempt(node.id, attempt_state_for(result), result)
        |> Graph.apply_output(node.id, result)

      :unknown ->
        strand_or_retry(graph, node)
    end
  end

  defp reconcile_subrun(%RunGraph{} = graph, %Node{} = node, store_opts) do
    case current_attempt(node) do
      %Attempt{engine: :subrun, thread_id: child_run_id} when is_binary(child_run_id) and child_run_id != "" ->
        case Store.load(child_run_id, store_opts) do
          {:ok, %RunGraph{status: status} = child} when status in [:succeeded, :failed, :cancelled] ->
            result = subrun_result(child)

            reconciled =
              graph
              |> mark_attempt(node.id, attempt_state_for(result), result)
              |> Graph.apply_output(node.id, result)

            {:ok, reconciled}

          {:ok, %RunGraph{}} ->
            {:ok, strand_or_retry(graph, node)}

          {:error, _reason} ->
            {:ok, strand_or_retry(graph, node)}
        end

      %Attempt{engine: :subrun} ->
        {:ok, strand_or_retry(graph, node)}

      _ ->
        :not_subrun
    end
  end

  defp subrun_result(%RunGraph{status: :succeeded} = child), do: {:ok, subrun_output(child)}

  defp subrun_result(%RunGraph{status: status, run_id: run_id}) when status in [:failed, :cancelled], do: {:error, {:subrun_failed, run_id, status}}

  defp subrun_output(%RunGraph{} = child) do
    %{
      kind: :subrun,
      run_id: child.run_id,
      status: child.status,
      outputs: node_outputs(child)
    }
  end

  defp node_outputs(%RunGraph{nodes: nodes}) do
    nodes
    |> Enum.filter(fn {_id, node} -> node.state == :succeeded end)
    |> Map.new(fn {id, node} -> {id, node.output} end)
  end

  # The thread is gone. The owning task died without a result, so the
  # attempt is stranded. Whether we auto-retry depends on the
  # non-idempotent safety rule.
  defp strand_or_retry(%RunGraph{} = graph, %Node{} = node) do
    graph = mark_attempt(graph, node.id, :stranded, :stranded)

    if auto_retryable?(node) do
      transition(graph, node.id, :retrying)
    else
      transition(graph, node.id, :stranded)
    end
  end

  # A bounded retry budget so a node that crashes deterministically does
  # not strand-retry forever. Conservative: a few attempts then human
  # review. The interpreter can carry a per-node override later; this is
  # the safe default.
  @max_attempts 3

  @doc """
  Whether a stranded node may be auto-retried. Conservative by default:
  the node must opt in (`inputs["__retry__"] == {:literal, true}`), its
  current attempt must show no observed side effect (no `thread_id`
  recorded), and it must be under the retry budget (`#{@max_attempts}`
  attempts). An attempt that opened an engine thread is assumed to have
  possibly acted and routes to human review instead.
  """
  @spec auto_retryable?(Node.t()) :: boolean()
  def auto_retryable?(%Node{} = node) do
    opted_in?(node) and not observed_side_effect?(node) and under_budget?(node)
  end

  defp under_budget?(%Node{attempts: attempts}), do: length(attempts) < @max_attempts

  defp opted_in?(%Node{inputs: inputs}) when is_map(inputs) do
    Map.get(inputs, "__retry__") == {:literal, true}
  end

  defp opted_in?(_), do: false

  # A recorded thread_id means an engine turn started, which may have
  # pushed a commit. That is the side effect the safety rule guards
  # against, so its presence blocks auto-retry.
  defp observed_side_effect?(%Node{} = node) do
    case current_attempt(node) do
      %Attempt{thread_id: thread_id} when is_binary(thread_id) and thread_id != "" -> true
      _ -> false
    end
  end

  defp current_attempt(%Node{attempts: []}), do: nil
  defp current_attempt(%Node{attempts: attempts}), do: Enum.max_by(attempts, & &1.n)

  defp current_thread_id(%Node{} = node) do
    case current_attempt(node) do
      %Attempt{thread_id: thread_id} -> thread_id
      nil -> nil
    end
  end

  defp transition(%RunGraph{} = graph, node_id, state) do
    case Map.fetch(graph.nodes, node_id) do
      {:ok, node} ->
        updated = %{node | state: state, updated_at: DateTime.utc_now()}
        %{graph | nodes: Map.put(graph.nodes, node_id, updated), updated_at: DateTime.utc_now()}

      :error ->
        graph
    end
  end

  defp mark_attempt(%RunGraph{} = graph, node_id, attempt_state, outcome) do
    case Map.fetch(graph.nodes, node_id) do
      {:ok, %Node{attempts: []} = node} ->
        # No attempt was ever recorded (persisted :running before the
        # attempt struct was appended). Synthesize one so the run record
        # still explains the strand.
        attempt = 1 |> Attempt.start(attempt_engine(node)) |> Attempt.finish(:stranded, :stranded)
        put_attempts(graph, node, [attempt])

      {:ok, node} ->
        attempts = update_current_attempt(node.attempts, attempt_state, finish_outcome(outcome))
        put_attempts(graph, node, attempts)

      :error ->
        graph
    end
  end

  defp put_attempts(%RunGraph{} = graph, %Node{} = node, attempts) do
    updated = %{node | attempts: attempts, updated_at: DateTime.utc_now()}
    %{graph | nodes: Map.put(graph.nodes, node.id, updated), updated_at: DateTime.utc_now()}
  end

  defp update_current_attempt(attempts, state, outcome) do
    current = Enum.max_by(attempts, & &1.n)
    finished = Attempt.finish(current, state, outcome)
    Enum.map(attempts, fn a -> if a.n == current.n, do: finished, else: a end)
  end

  defp attempt_engine(%Node{envelope: %{engine: engine}}) when engine in [:codex, :claude, :pi], do: engine

  defp attempt_engine(_node), do: :codex

  defp attempt_state_for({:ok, _}), do: :succeeded
  defp attempt_state_for({:error, _}), do: :failed

  # The Attempt state is one of :succeeded | :failed; pick from the result.
  defp finish_outcome(:stranded), do: :stranded
  defp finish_outcome({:ok, _}), do: :ok
  defp finish_outcome({:error, reason}), do: {:error, reason}
end
