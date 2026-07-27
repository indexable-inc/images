defmodule IxMcp.Dashboard.Bridge do
  @moduledoc """
  Holds the dashboard documents' native watch streams and turns each change
  into a durable notification for every session viewing that document.

  Why one process for all documents: the `:dashboard_ex` NIF pushes stream
  items to `env.pid()`, fixed at the moment `DashboardEx.Native.watch/2` is
  called, and there is no arbitrary-pid send. So the subscription has to be
  taken from the process that will receive it, and fan-out to the other
  interested sessions has to happen on the BEAM side. This is that process.

  Why `watch_stream/1` and not the generated `DashboardEx.watch/1`: the
  latter is an `Enumerable` that blocks on a bare `receive`, which owns the
  calling process and so is unusable from a GenServer. `watch_stream/1`
  returns a `%DashboardEx.StreamHandle{}` instead and never blocks. The
  handle is kept in state because collecting it aborts the producer -- which
  is also how `unview/2` stops a stream once its last viewer leaves.

  Why the `{:unibind_stream, ref, _}` tuple still appears in a `handle_info`
  head: `stream_message/2` classifies a message for *one* stream, and this
  process holds one per document. Matching the ref in the head keeps the
  dispatch a single map lookup rather than a scan over every open stream;
  the payload itself is still classified by `stream_message/2` rather than
  decoded here.

  Why the outbox rather than `Notifier.channel/2`: `channel/2` drops the
  message when no transport is connected and broadcasts to every connected
  session when one is (#3934). A viewer that stepped away has to learn about
  the edit on reconnect, and a session that is not watching the document must
  not hear about it at all -- so each edit becomes one durable outbox row per
  subscribed session, published to that session and replayed by
  `Notifier.register/1` if it was away.

  The outbox is job-shaped (it is the job ledger's notification table), so a
  row is written through `ActionLog.finish_job/5` with a synthetic job id:
  that is the only public way to get a durable, session-scoped row, and
  `unacked_outbox/1` joins `jobs` to find the session. A notification kind
  that is not a job is the proper fix and is follow-up work; the wart is one
  history entry per coalesced edit window per viewer.
  """

  use GenServer

  alias IxMcp.ActionLog
  alias IxMcp.MCP.Notifier
  alias IxMcp.NifApp

  # Referenced as an atom so compiling this app never needs the NIF app
  # present: it is loaded at runtime from IX_MCP_DASHBOARD_EX.
  @native :"Elixir.DashboardEx"
  @app :dashboard_ex
  @env_var "IX_MCP_DASHBOARD_EX"

  # One Loro commit is several container diffs, and a human typing produces a
  # commit per keystroke burst, so a row per item would drown the ledger.
  # Long enough to fold a sentence's worth of typing into one notification,
  # short enough that a wake still feels immediate. Tests shrink it via app
  # env.
  @coalesce_ms Application.compile_env(:ix_mcp, :dashboard_coalesce_ms, 500)

  @spec start_link(term()) :: GenServer.on_start()
  def start_link(_opts) do
    GenServer.start_link(__MODULE__, [], name: __MODULE__)
  end

  @doc """
  Subscribe `session_id` to `doc`'s changes, starting the native watch on
  first interest. Idempotent per session.
  """
  @spec view(String.t(), integer()) :: :ok | {:error, String.t()}
  def view(doc, session_id) when is_binary(doc) and is_integer(session_id) do
    GenServer.call(__MODULE__, {:view, doc, session_id})
  end

  @doc """
  Drop `session_id`'s interest in `doc`. The native watch stops once the last
  viewer leaves.
  """
  @spec unview(String.t(), integer()) :: :ok
  def unview(doc, session_id) when is_binary(doc) and is_integer(session_id) do
    GenServer.call(__MODULE__, {:unview, doc, session_id})
  end

  @doc "Session ids currently viewing `doc`, sorted."
  @spec viewers(String.t()) :: [integer()]
  def viewers(doc) when is_binary(doc) do
    GenServer.call(__MODULE__, {:viewers, doc})
  end

  @impl true
  def init(_opts) do
    {:ok, %{watches: %{}, refs: %{}, viewers: %{}, pending: %{}, timers: %{}}}
  end

  @impl true
  def handle_call({:view, doc, session_id}, _from, state) do
    case ensure_watch(state, doc) do
      {:ok, state} ->
        viewers =
          Map.update(state.viewers, doc, MapSet.new([session_id]), &MapSet.put(&1, session_id))

        {:reply, :ok, %{state | viewers: viewers}}

      {:error, reason} ->
        {:reply, {:error, reason}, state}
    end
  end

  def handle_call({:unview, doc, session_id}, _from, state) do
    remaining = state.viewers |> Map.get(doc, MapSet.new()) |> MapSet.delete(session_id)
    state = %{state | viewers: Map.put(state.viewers, doc, remaining)}

    state =
      if MapSet.size(remaining) == 0 do
        drop_watch(state, doc)
      else
        state
      end

    {:reply, :ok, state}
  end

  def handle_call({:viewers, doc}, _from, state) do
    {:reply, state.viewers |> Map.get(doc, MapSet.new()) |> Enum.sort(), state}
  end

  @impl true
  def handle_info({:unibind_stream, ref, _payload} = message, state) do
    case Map.fetch(state.watches, ref) do
      {:ok, watch} -> {:noreply, stream_event(state, watch, message)}
      # A stream whose document was already closed: nothing to credit and
      # nobody to tell.
      :error -> {:noreply, state}
    end
  end

  def handle_info({:flush, doc}, state) do
    {events, pending} = Map.pop(state.pending, doc, [])
    state = %{state | pending: pending, timers: Map.delete(state.timers, doc)}
    announce_all(doc, Enum.reverse(events), Map.get(state.viewers, doc, MapSet.new()))
    {:noreply, state}
  end

  def handle_info(_message, state), do: {:noreply, state}

  defp stream_event(state, watch, message) do
    case native(:stream_message, [watch.stream, message]) do
      {:item, event} ->
        # Re-credit before buffering: the producer is idle until it holds a
        # credit, and an announcement is never worth stalling the stream for.
        native(:stream_demand, [watch.stream, 1])
        buffer(state, watch.doc, event)

      :done ->
        drop_watch(state, watch.doc)

      :nomatch ->
        state
    end
  end

  # -- watches -----------------------------------------------------------------

  defp ensure_watch(state, doc) do
    if Map.has_key?(state.refs, doc) do
      {:ok, state}
    else
      start_watch(state, doc)
    end
  end

  defp start_watch(state, doc) do
    with :ok <- NifApp.ensure_loaded(@native, @app, @env_var) do
      # `watch_stream/1` fixes the destination pid to this process, so it has
      # to be called here rather than in the caller of `view/2`.
      case native(:watch_stream, [doc]) do
        {:ok, %{ref: ref} = stream} ->
          native(:stream_demand, [stream, 1])

          {:ok,
           %{
             state
             | refs: Map.put(state.refs, doc, ref),
               watches: Map.put(state.watches, ref, %{doc: doc, stream: stream})
           }}

        {:error, error} ->
          {:error, describe(error)}
      end
    end
  end

  # Forgetting the stream handle is what stops the producer: the BEAM
  # collects the last reference to it and the NIF aborts the stream, which
  # drops the Loro subscription with it.
  defp drop_watch(state, doc) do
    {ref, refs} = Map.pop(state.refs, doc)
    cancel_timer(state.timers[doc])

    %{
      state
      | refs: refs,
        watches: Map.delete(state.watches, ref),
        viewers: Map.delete(state.viewers, doc),
        pending: Map.delete(state.pending, doc),
        timers: Map.delete(state.timers, doc)
    }
  end

  defp cancel_timer(nil), do: :ok
  defp cancel_timer(timer), do: Process.cancel_timer(timer)

  # `apply/3` through variables on purpose: the generated namespace is
  # referenced as an atom (it is loaded at runtime), so a direct call would
  # be a compile-time warning in a warnings-as-errors app.
  defp native(fun, args), do: apply(@native, fun, args)

  defp describe(%{message: message}) when is_binary(message), do: message
  defp describe(other), do: inspect(other)

  # -- coalescing and fan-out --------------------------------------------------

  defp buffer(state, doc, event) do
    pending = Map.update(state.pending, doc, [event], &[event | &1])

    timers =
      Map.put_new_lazy(state.timers, doc, fn ->
        Process.send_after(self(), {:flush, doc}, @coalesce_ms)
      end)

    %{state | pending: pending, timers: timers}
  end

  defp announce_all(_doc, [], _viewers), do: :ok

  defp announce_all(doc, events, viewers) do
    summary = summarize(doc, events)
    Enum.each(viewers, &announce(doc, &1, summary))
  end

  defp summarize(doc, events) do
    roots = events |> Enum.map(& &1.root) |> Enum.uniq() |> Enum.sort() |> Enum.join(", ")
    inserted = events |> Enum.map(& &1.inserted) |> Enum.sum()
    deleted = events |> Enum.map(& &1.deleted) |> Enum.sum()

    paths =
      events |> Enum.map(& &1.path) |> Enum.reject(&(&1 == "")) |> Enum.uniq() |> Enum.take(5)

    line =
      "dashboard #{doc}: #{length(events)} change(s) under #{roots} (+#{inserted}/-#{deleted} chars)"

    if paths == [], do: line, else: line <> " at " <> Enum.join(paths, ", ")
  end

  # One durable row per viewer, then a publish scoped to that viewer's
  # session. The ledger must never take the bridge down with it: a failed
  # write costs one notification, a crash costs every future one.
  defp announce(doc, session_id, summary) do
    id = "dashboard-#{doc}-#{System.unique_integer([:positive, :monotonic])}"

    start = %{
      id: id,
      session_id: session_id,
      action_id: nil,
      intent: summary,
      session_name: nil,
      topic_name: nil,
      code: "",
      watch: false,
      started_at: DateTime.to_iso8601(DateTime.utc_now())
    }

    case ActionLog.finish_job(id, :done, summary, start: start) do
      {:notify, outbox} -> Notifier.publish(outbox)
      :already_final -> :ok
    end
  catch
    :exit, _reason -> :ok
  end
end
