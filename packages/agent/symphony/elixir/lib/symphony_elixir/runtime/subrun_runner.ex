defmodule SymphonyElixir.Runtime.SubrunRunner do
  @moduledoc """
  Executor for `:subrun` IR nodes: a first-class nested run. This is the
  IR-era replacement for the `{:error, {:subrun_unsupported, id}, nil}`
  stub the runtime carried while the rest of the substrate landed.

  A subrun node names a child workflow (`subrun "child.sym"`) and optional
  extra inputs. The runner resolves the child against `WorkflowCatalog`,
  starts it through `Runtime.Ingress.start_workflow/3` under the same
  supervisor, waits for the child to settle, and maps the child's terminal
  `RunGraph.status` back to the one result triple every node kind returns
  (`{:ok, output, nil}` / `{:error, reason, nil}`). A subrun has no engine
  thread of its own, so `thread_id` is always `nil`.

  ## Guarding recursion

  Two distinct guards keep a `subrun` from spawning an unbounded tree:

  - A cycle guard rejects a child whose workflow name is already on the
    ancestor chain (`{:subrun_cycle, name, chain}`). This catches direct
    self-subruns and any loop back to a workflow already running above.
  - A depth ceiling rejects a chain longer than `Config.subrun_max_depth`
    (`{:subrun_depth_exceeded, depth, ceiling}`). This is the backstop the
    cycle guard cannot provide: two workflows that call each other through
    a third, or any mutually recursive set with no repeated name on a
    single branch, would otherwise recurse forever.

  Both the depth and the ancestor chain are threaded down through
  `run_opts`; the parent runtime carries its own depth and chain in process
  state and stamps the child's onto the start opts, so a child run knows
  exactly how deep it sits and which workflows are open above it.

  ## Why monitor instead of message

  The child run is a supervised `Runtime` GenServer. A succeeded or
  cancelled run stops its process; a failed run stays alive and idle so the
  operator surface can reach it. The runner monitors the child pid and, on
  the `:DOWN` (or immediately for a run that started already-failed), reads
  the durable terminal graph from `IR.Store` rather than trusting an
  in-memory snapshot, so the mapped result reflects the persisted truth.
  """

  alias SymphonyElixir.Config
  alias SymphonyElixir.IR.Node
  alias SymphonyElixir.IR.RunGraph
  alias SymphonyElixir.IR.Store
  alias SymphonyElixir.Runtime
  alias SymphonyElixir.WorkflowCatalog

  require Logger

  @type result :: {:ok, map(), nil} | {:error, term(), nil}

  # A child that started :failed (or a graph that resolves the moment it is
  # scheduled) never emits a :DOWN we are waiting on, so cap the wait and
  # fall back to a store read. The ceiling is generous: a real child run is
  # bounded by its own node executors' timeouts, not this value.
  @child_wait_ms 24 * 60 * 60 * 1_000

  @spec run(Node.t(), map()) :: result()
  def run(%Node{kind: :subrun, inputs: inputs} = node, run_opts) when is_map(run_opts) do
    with {:ok, name} <- fetch_child_name(node, run_opts),
         :ok <- check_depth(run_opts),
         :ok <- check_cycle(name, run_opts),
         {:ok, entry} <- resolve_workflow(name) do
      start_and_await(entry, name, inputs, run_opts)
    else
      {:error, reason} -> {:error, reason, nil}
    end
  end

  # The child workflow name lives on the node's `source` input. A static
  # `subrun "child.sym"` lowers to `{:literal, "child.sym"}`; a source read
  # from an upstream output is resolved by the runtime into `resolved_inputs`
  # before scheduling, so a dynamic source name is read from there. The
  # catalog keys workflows by basename without the `.sym` extension, so the
  # name is normalized the same way the author writes the file.
  defp fetch_child_name(%Node{inputs: inputs, id: id}, run_opts) do
    resolved = Map.get(run_opts, :resolved_inputs, %{})

    raw =
      case Map.get(resolved, "source") do
        value when is_binary(value) -> value
        _ -> literal_source(inputs)
      end

    case normalize_name(raw) do
      name when is_binary(name) and name != "" -> {:ok, name}
      _ -> {:error, {:subrun_missing_source, id}}
    end
  end

  defp literal_source(inputs) do
    case Map.get(inputs, "source") do
      {:literal, value} when is_binary(value) -> value
      _ -> nil
    end
  end

  defp normalize_name(value) when is_binary(value) do
    value
    |> String.trim()
    |> String.replace_suffix(".sym", "")
  end

  defp normalize_name(_), do: nil

  # The child sits one level below the parent. Reject before starting any
  # child process so an over-deep chain never allocates a run id or touches
  # the store.
  defp check_depth(run_opts) do
    depth = current_depth(run_opts)
    ceiling = subrun_ceiling()

    if depth + 1 > ceiling do
      {:error, {:subrun_depth_exceeded, depth + 1, ceiling}}
    else
      :ok
    end
  end

  # A child whose name is already open above us would loop. Self-subruns are
  # the first element of this set; a longer cycle is caught the same way
  # because every ancestor name is on the chain.
  defp check_cycle(name, run_opts) do
    chain = ancestors(run_opts)

    if name in chain do
      {:error, {:subrun_cycle, name, chain}}
    else
      :ok
    end
  end

  defp resolve_workflow(name) do
    case WorkflowCatalog.workflow(name) do
      {:ok, entry} -> {:ok, entry}
      {:error, :not_found} -> {:error, {:subrun_workflow_not_found, name}}
    end
  end

  defp start_and_await(entry, name, inputs, run_opts) do
    trigger = trigger_context(inputs, run_opts)
    child_opts = child_opts(name, run_opts)

    case Runtime.Ingress.start_workflow(entry, trigger, child_opts) do
      {:ok, %{run_id: run_id, pid: pid}} ->
        notify_child_started(run_id, run_opts)
        await_child(run_id, pid, run_opts)

      {:error, reason} ->
        {:error, {:subrun_start_failed, name, reason}, nil}
    end
  end

  defp notify_child_started(run_id, run_opts) do
    case Map.get(run_opts, :on_child_started) do
      callback when is_function(callback, 1) -> callback.(run_id)
      _ -> :ok
    end
  end

  # The extra `subrun "child" { k: v }` bindings become the child's trigger
  # context, the same `<input>` surface a producer-started run reads. The
  # resolved values (from upstream outputs) take precedence over the
  # unresolved literal refs; `source` is the workflow selector, not run
  # input, so it is dropped from the context.
  defp trigger_context(inputs, run_opts) do
    resolved = Map.get(run_opts, :resolved_inputs, %{})

    literals =
      for {key, {:literal, value}} <- inputs, key != "source", into: %{}, do: {key, value}

    context = Map.merge(literals, Map.delete(resolved, "source"))

    if context == %{}, do: nil, else: context
  end

  # Thread the engine and store through so the child runs against the same
  # injected engine and store dir as the parent (tests inject a fake; one
  # store dir keeps the child's terminal graph readable here). Push the
  # child's depth and ancestor chain so its own subruns guard correctly.
  defp child_opts(name, run_opts) do
    chain = [name | ancestors(run_opts)]
    depth = current_depth(run_opts) + 1

    []
    |> put_if_present(:engine, Map.get(run_opts, :engine))
    |> put_if_present(:store_opts, Map.get(run_opts, :store_opts))
    |> Keyword.put(:subrun_depth, depth)
    |> Keyword.put(:subrun_ancestors, chain)
  end

  defp put_if_present(opts, _key, nil), do: opts
  defp put_if_present(opts, key, value), do: Keyword.put(opts, key, value)

  defp await_child(run_id, pid, run_opts) do
    ref = Process.monitor(pid)

    receive do
      {:DOWN, ^ref, :process, ^pid, _reason} -> finalize(run_id, run_opts)
    after
      @child_wait_ms ->
        Process.demonitor(ref, [:flush])
        # A child that outlives the ceiling is itself a runaway; surface it
        # rather than block the parent's task forever.
        {:error, {:subrun_timeout, run_id}, nil}
    end
  end

  # The child stopped (succeeded/cancelled) or never moved from its
  # already-terminal start (a failed run stays alive). Either way the
  # durable graph in the store is the source of truth for the mapped
  # result. A failed run keeps a live process, so demonitor cannot have
  # fired for it; the store read still resolves it.
  defp finalize(run_id, run_opts) do
    store_opts = Map.get(run_opts, :store_opts, [])

    case Store.load(run_id, store_opts) do
      {:ok, %RunGraph{} = graph} -> map_terminal(graph)
      {:error, reason} -> {:error, {:subrun_result_unavailable, run_id, reason}, nil}
    end
  end

  defp map_terminal(%RunGraph{status: :succeeded} = graph) do
    {:ok, child_output(graph), nil}
  end

  defp map_terminal(%RunGraph{status: status, run_id: run_id}) when status in [:failed, :cancelled] do
    {:error, {:subrun_failed, run_id, status}, nil}
  end

  defp map_terminal(%RunGraph{status: status, run_id: run_id}) do
    # A non-terminal status here means the child stopped without resolving,
    # which should not happen for a settled run; treat it as a failure
    # rather than a silent success.
    {:error, {:subrun_unresolved, run_id, status}, nil}
  end

  # The subrun node's output is the child run's terminal facts: its id,
  # status, and the per-node outputs the child produced. The full child
  # graph stays in its own run file; this is the compact result a parent
  # node downstream reads through its inputs.
  defp child_output(%RunGraph{} = graph) do
    %{
      kind: :subrun,
      run_id: graph.run_id,
      status: graph.status,
      outputs: node_outputs(graph)
    }
  end

  defp node_outputs(%RunGraph{nodes: nodes}) do
    for {id, %Node{output: output}} <- nodes, not is_nil(output), into: %{}, do: {id, output}
  end

  defp current_depth(run_opts) do
    case Map.get(run_opts, :subrun_depth) do
      depth when is_integer(depth) and depth >= 0 -> depth
      _ -> 0
    end
  end

  defp ancestors(run_opts) do
    case Map.get(run_opts, :subrun_ancestors) do
      chain when is_list(chain) -> chain
      _ -> []
    end
  end

  # The configured ceiling, with a conservative fallback when Config is not
  # running (some unit tests start the runner without the full app tree).
  defp subrun_ceiling do
    Config.get().subrun_max_depth
  rescue
    _ -> 8
  end
end
