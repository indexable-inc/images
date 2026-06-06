defmodule SymphonyElixir.Runtime.Ingress do
  @moduledoc """
  The single door that turns a workflow plus a trigger event into a live IR
  run. A producer (cron, a webhook, the enqueue UI) resolves an event to a
  `WorkflowCatalog` entry, then calls here.

  `start_workflow/3` materializes the workflow's AST into a `RunGraph`
  (validating envelopes at load), stamps the trigger event onto the graph
  so a node can read it as `<input>` context, and starts the run under
  `Runtime.Supervisor`. The source hash recorded on the run is the
  catalog's hash of the `.sym` bytes, so editing the pack never perturbs a
  run already in flight.

  The engine and store options pass straight through to the supervisor, so
  a test injects a fake engine and an isolated store dir the same way the
  runtime tests do.
  """

  alias SymphonyElixir.IR.{Materializer, RunGraph, Store}
  alias SymphonyElixir.Runtime
  alias SymphonyElixir.Runtime.Trigger
  alias SymphonyElixir.WorkflowCatalog

  @typedoc "A started run: its generated id and the supervised runtime pid."
  @type started :: %{run_id: String.t(), pid: pid()}

  @doc """
  Materialize a catalog entry and start it. `trigger_context` is the event
  payload (`nil` for an operator-started run); `opts` forwards `:engine`,
  `:store_opts`, and an optional `:run_id` to the supervisor.
  """
  @spec start_workflow(WorkflowCatalog.entry(), map() | nil, keyword()) :: {:ok, started()} | {:error, term()}
  def start_workflow(entry, trigger_context \\ nil, opts \\ [])

  def start_workflow(%{ast: ast, hash: hash} = entry, trigger_context, opts) do
    run_id = Keyword.get_lazy(opts, :run_id, fn -> generate_run_id(entry) end)
    start_opts = Keyword.drop(opts, [:run_id])

    with {:ok, graph} <- Materializer.materialize(run_id, hash, ast) do
      graph = %{graph | trigger: trigger_context}

      case Runtime.Supervisor.start_run(graph, start_opts) do
        {:ok, pid} -> {:ok, %{run_id: run_id, pid: pid}}
        {:error, _} = err -> err
      end
    end
  end

  @doc """
  Resolve every `.sym` workflow that declared interest in this trigger
  event and start one IR run per match, carrying the event as the run's
  trigger context.

  This is the one ingress door for every event-driven producer (cron, the
  webhooks, the Slack poller, the HTTP API). The producer owns event
  extraction, signature verification, and dedup; this owns resolution and
  start. Candidates come from `WorkflowCatalog.for_trigger_kind/1` (the
  cheap kind filter) and are narrowed by the shared `Runtime.Trigger`
  matcher, so the selector vocabulary lives in one module rather than
  re-implemented per producer.

  Returns `{:ok, [started()]}` with one entry per started run (an empty
  list when no workflow matched, which is not an error: an event with no
  interested workflow is a no-op). Returns `{:error, {:partial, started,
  failures}}` if any matched workflow failed to start, after starting the
  ones that could, so a single bad workflow does not silence the rest.

  `opts` forwards `:engine` and `:store_opts` to the supervisor; in
  production both default (the room-server client and the configured runs
  dir), so a producer calls `start_by_trigger(event)` with no opts.
  """
  @spec start_by_trigger(map(), keyword()) :: {:ok, [started()]} | {:error, term()}
  def start_by_trigger(%{kind: kind} = event, opts \\ []) when is_atom(kind) do
    kind
    |> WorkflowCatalog.for_trigger_kind()
    |> Enum.filter(fn entry -> Trigger.matches?(entry.trigger, event) end)
    |> Enum.reduce({[], []}, fn entry, {started, failures} ->
      case start_workflow(entry, event, opts) do
        {:ok, run} -> {[run | started], failures}
        {:error, reason} -> {started, [{entry.name, reason} | failures]}
      end
    end)
    |> case do
      {started, []} -> {:ok, Enum.reverse(started)}
      {started, failures} -> {:error, {:partial, Enum.reverse(started), Enum.reverse(failures)}}
    end
  end

  @doc "Resolve a workflow by catalog name, then start it. Convenience for the manual/enqueue path."
  @spec start_by_name(String.t(), map() | nil, keyword()) :: {:ok, started()} | {:error, term()}
  def start_by_name(name, trigger_context \\ nil, opts \\ []) when is_binary(name) do
    case WorkflowCatalog.workflow(name) do
      {:ok, entry} -> start_workflow(entry, trigger_context, opts)
      {:error, :not_found} -> {:error, {:workflow_not_found, name}}
    end
  end

  @doc """
  Has any IR run already been started for `trigger` events that satisfy
  `match_fun`? The dedup read every event-driven producer shares.

  A producer keeps its own field-level predicate (a GitHub run dedups on
  `repo`/`pr_number`, a Slack huddle on `message_ts`), and this owns the
  one shared capability: where the run history lives. Runs are
  `RunGraph`s, so the history is `IR.Store`. `match_fun` receives
  `{status, trigger}` for every persisted IR run, so a caller can scope to
  active runs (`status in [:pending, :running]`) or to any run.

  `opts` forwards `:store_opts` so a test points the read at an isolated
  store dir.
  """
  @spec seen_trigger?(({RunGraph.status(), map() | nil} -> boolean()), keyword()) :: boolean()
  def seen_trigger?(match_fun, opts \\ []) when is_function(match_fun, 1) do
    store_opts = Keyword.get(opts, :store_opts, [])

    store_opts
    |> Store.load_all()
    |> Enum.any?(fn graph -> match_fun.({graph.status, graph.trigger}) end)
  end

  # A readable, collision-resistant run id: the workflow slug, the wall
  # clock, and a monotonic counter. Ids are opaque to the store; the slug is
  # only there to make a runs listing scannable.
  defp generate_run_id(%{name: name}) do
    slug =
      name
      |> to_string()
      |> String.downcase()
      |> String.replace(~r/[^a-z0-9]+/, "-")
      |> String.trim("-")

    slug = if slug == "", do: "workflow", else: slug
    "#{slug}-#{System.system_time(:millisecond)}-#{System.unique_integer([:positive, :monotonic])}"
  end
end
