defmodule IxMcp.MCP.Notifier do
  @moduledoc """
  Fan-out point for server-initiated MCP notifications. Session wakes ride
  the Claude Code channel contract: `notifications/claude/channel` events,
  paired with the experimental `claude/channel` capability the server
  declares at initialize. Claude Code receives `notifications/message` (MCP
  logging) but never surfaces it, so nothing user-facing may depend on that
  method (#3785). Every channel message carries `meta.severity` (`"info"` |
  `"failure"`) so a harness can render failures loudly and mute the rest
  (#3934).

  Notifications derive from durable transitions (#3839). A terminal job
  transition inserts a row into the `outbox` table (part of the same atomic
  write as the transition); `publish/1` queues it for the transports of the
  session that owns the job -- live delivery is scoped exactly like the
  reconnect replay, where an unscoped broadcast buried the one message that
  matters under every other finish (#3934). Queued rows coalesce per session
  for a short window and leave as one message: one line for a lone finish,
  a replay-shaped digest beyond that. Delivery re-checks each row against
  the ledger, so a row the exec reply path already acked -- the job finished
  within its budget and the caller got the result in the tool reply -- is
  skipped silently. When no transport is connected the rows stay unacked and
  `register/1` replays them as one digest the moment a transport (re)joins:
  the no-silent-death invariant is the durable row, not a broadcast.

  Cross-session interest is explicit rather than ambient (#3934):
  `watch/2` subscribes a session to another job's or session's terminal
  transitions, observed by polling the shared ledger -- the watched job may
  live in a sibling kernel instance, so no in-memory signal can cross.
  """

  use GenServer

  alias IxMcp.ActionLog
  alias IxMcp.Session

  # How long finishes for one session pool before leaving as a single
  # message (#3934): long enough to fold an await wrapper's target and any
  # sibling finishes into one digest, and for the exec reply path to ack a
  # within-budget job before its row is ever rendered; short enough that a
  # background death still lands promptly. Tests shrink it via app env.
  @coalesce_ms Application.compile_env(:ix_mcp, :notify_coalesce_ms, 2_000)

  # Watch-poll cadence. Watches read the shared database, so the interval
  # is also the cross-instance notification latency.
  @watch_poll_ms Application.compile_env(:ix_mcp, :watch_poll_ms, 2_000)

  @typedoc "A watch target: one job, or every job of a session."
  @type watch_target :: {:job, String.t()} | {:session, integer()}

  @spec start_link(term()) :: GenServer.on_start()
  def start_link(_opts) do
    GenServer.start_link(__MODULE__, [], name: __MODULE__)
  end

  @doc """
  Register a transport for the current session. Any outbox notifications
  that fired while no transport was connected are replayed at once as a
  single digest.
  """
  @spec register(pid()) :: :ok
  def register(transport) when is_pid(transport) do
    GenServer.cast(__MODULE__, {:register, transport})
  end

  @doc """
  Queue a terminal-transition notification for the owning session's
  transports. Coalesced with the session's other finishes for a short
  window; with no transport connected it is dropped here and the outbox row
  waits for `register/1` to replay it.
  """
  @spec publish(ActionLog.outbox()) :: :ok
  def publish(outbox) do
    case Process.whereis(__MODULE__) do
      nil -> :ok
      pid -> GenServer.cast(pid, {:publish, outbox})
    end
  end

  @doc """
  Subscribe `watcher_session_id` to a target's terminal transitions
  (#3934): every hit is announced to that session's transports, never
  acked -- the outbox row still belongs to the owning session. A job watch
  fires once; a session watch keeps announcing that session's finishes.
  """
  @spec watch(integer(), watch_target()) :: :ok
  def watch(watcher_session_id, target)
      when is_integer(watcher_session_id) and elem(target, 0) in [:job, :session] do
    GenServer.call(__MODULE__, {:watch, watcher_session_id, target})
  end

  @typedoc """
  Wire meta: string keys to string values, no exceptions. A key with nothing
  to say is left out, never sent as nil.
  """
  @type meta :: %{optional(String.t()) => String.t()}

  @doc """
  Push one event into the connected Claude session. `content` becomes the
  body of the `<channel>` tag the client injects; each `meta` entry becomes
  a tag attribute, so values are short strings. `severity` defaults to
  `"info"`; pass `"failure"` for events a harness should render loudly.
  """
  @spec channel(String.t(), meta()) :: :ok
  def channel(content, meta) do
    meta = Map.put_new(meta, "severity", "info")

    notify("notifications/claude/channel", %{
      "content" => content,
      "meta" => strings_only!(meta, content)
    })
  end

  # The client parses meta as string-to-string and drops the whole event on
  # anything else -- an integer or a nil costs the notification, and nothing
  # reaches the sender to say so: the ledger row is written and acked, the
  # session simply never hears. Raising puts the failure at the producer's
  # own call site, where its test sees it, rather than in an operator's
  # missing wake. Raised in the caller's process on purpose: the Notifier
  # holds every session's coalesce buffer and its transport registry, so a
  # crash there would silence more than the one bad event.
  @spec strings_only!(meta(), String.t()) :: meta()
  defp strings_only!(meta, content) do
    case Enum.reject(meta, fn {_key, value} -> is_binary(value) end) do
      [] ->
        meta

      bad ->
        raise ArgumentError,
              "channel meta values must be strings, got #{inspect(Map.new(bad))} " <>
                "on #{inspect(String.slice(content, 0, 60))}"
    end
  end

  @doc """
  How many transports are attached right now.

  Fleet alerts (ENG-11209) ask before writing their durable "already
  announced" fingerprint: unlike a job finish, a fleet hit has no outbox row,
  so recording it with nobody listening would bury the fault permanently
  rather than replay it on reconnect.
  """
  @spec transports() :: non_neg_integer()
  def transports do
    case Process.whereis(__MODULE__) do
      nil -> 0
      pid -> GenServer.call(pid, :transport_count)
    end
  catch
    :exit, _reason -> 0
  end

  @spec notify(String.t(), map()) :: :ok
  def notify(method, params) do
    case Process.whereis(__MODULE__) do
      nil -> :ok
      pid -> GenServer.cast(pid, {:notify, method, params})
    end
  end

  @impl true
  def init(_) do
    {:ok,
     %{
       transports: %{},
       pending: %{},
       timers: %{},
       watches: %{},
       watch_timer: nil
     }}
  end

  @impl true
  def handle_cast({:register, transport}, state) do
    Process.monitor(transport)
    session_id = Session.ids().session_id
    state = %{state | transports: Map.put(state.transports, transport, session_id)}
    # A connecting transport is proof of life: stamp the session-directory
    # heartbeat (#3881) here, without waiting for the first watch tick.
    ActionLog.heartbeat_session(session_id)
    replay_unacked([transport], session_id)
    {:noreply, state}
  end

  def handle_cast({:publish, outbox}, state) do
    {:noreply, enqueue(state, outbox.session_id, %{kind: :outbox, row: outbox})}
  end

  def handle_cast({:notify, method, params}, state) do
    deliver(Map.keys(state.transports), method, params)
    {:noreply, state}
  end

  @impl true
  def handle_call(:transport_count, _from, state) do
    {:reply, map_size(state.transports), state}
  end

  def handle_call({:watch, watcher, target}, _from, state) do
    watches =
      Map.update(state.watches, watcher, add_watch(empty_watch(), target), &add_watch(&1, target))

    {:reply, :ok, arm_watch_timer(%{state | watches: watches})}
  end

  @impl true
  def handle_info({:DOWN, _ref, :process, pid, _reason}, state) do
    {:noreply, %{state | transports: Map.delete(state.transports, pid)}}
  end

  # One session's coalesce window closed: deliver everything it pooled as a
  # single message. Deliver before acking (#3874): an ack that committed
  # without its delivery would silence the notification permanently on
  # retry -- the exact silence the outbox exists to prevent. Publish,
  # replay, and this flush all run in this process, so re-checking rows
  # against the ledger just before delivering keeps the same-finish dedup
  # (#3839) race-free -- and it is also the suppression seam (#3934): a row
  # the exec reply path acked in the window simply no longer qualifies. The
  # residual worst case is a duplicate announce, which beats a lost one.
  # With no transport connected nothing is delivered or acked -- unacked
  # rows wait for replay; watch hits are best-effort and drop.
  def handle_info({:flush, session_id}, state) do
    {entries, pending} = Map.pop(state.pending, session_id, [])
    state = %{state | pending: pending, timers: Map.delete(state.timers, session_id)}

    transports = transports_for(state, session_id)
    live = if transports == [], do: [], else: deliverable(dedup(entries))

    if live != [] do
      {content, meta} = render_entries(live)

      deliver(transports, "notifications/claude/channel", %{
        "content" => content,
        "meta" => meta
      })

      live
      |> Enum.filter(&(&1.kind == :outbox))
      |> Enum.map(& &1.row.id)
      |> ack_all()
    end

    {:noreply, state}
  end

  def handle_info(:poll_watches, state) do
    state = poll_watches(%{state | watch_timer: nil})
    {:noreply, arm_watch_timer(state)}
  end

  def handle_info(_msg, state), do: {:noreply, state}

  # -- coalescing --------------------------------------------------------------

  defp enqueue(state, session_id, entry) do
    pending = Map.update(state.pending, session_id, [entry], &(&1 ++ [entry]))

    timers =
      Map.put_new_lazy(state.timers, session_id, fn ->
        Process.send_after(self(), {:flush, session_id}, @coalesce_ms)
      end)

    %{state | pending: pending, timers: timers}
  end

  # An await wrapper's target can arrive both as the owner's outbox row and
  # as a watch hit in the same window; the owned row wins (it acks).
  defp dedup(entries) do
    {outbox, watch} = Enum.split_with(entries, &(&1.kind == :outbox))
    owned = MapSet.new(outbox, & &1.row.job_id)

    watch =
      watch
      |> Enum.reject(&MapSet.member?(owned, &1.row.job_id))
      |> Enum.uniq_by(& &1.row.job_id)

    outbox ++ watch
  end

  # A transport registered before its session acted has the session the
  # registration created; a nil-session outbox row (a pre-#3934 database)
  # can only be this instance's own work, so it goes to every transport.
  defp transports_for(state, nil), do: Map.keys(state.transports)

  defp transports_for(state, session_id) do
    for {pid, sid} <- state.transports, sid == session_id, do: pid
  end

  defp deliverable(entries) do
    unacked = unacked_ids()
    Enum.filter(entries, &(&1.kind == :watch or MapSet.member?(unacked, &1.row.id)))
  end

  defp unacked_ids do
    MapSet.new(ActionLog.unacked_outbox(), & &1.id)
  catch
    # Ledger down: no row can be confirmed undelivered, so deliver none --
    # they stay unacked and replay once the ledger returns (#3874).
    :exit, _reason -> MapSet.new()
  end

  # -- watches (#3934) ---------------------------------------------------------

  defp empty_watch, do: %{jobs: MapSet.new(), sessions: %{}}

  defp add_watch(watch, {:job, id}), do: %{watch | jobs: MapSet.put(watch.jobs, id)}

  # The watermark starts at subscription time: a session watch means "from
  # now on", not a replay of the watched session's history.
  defp add_watch(watch, {:session, sid}) do
    %{watch | sessions: Map.put_new(watch.sessions, sid, now_iso())}
  end

  defp arm_watch_timer(%{watch_timer: nil} = state) when map_size(state.watches) > 0 do
    %{state | watch_timer: Process.send_after(self(), :poll_watches, @watch_poll_ms)}
  end

  defp arm_watch_timer(state), do: state

  defp poll_watches(state) do
    Enum.reduce(state.watches, state, fn {watcher, watch}, state ->
      {state, jobs} = poll_job_watches(state, watcher, watch.jobs)
      {state, sessions} = poll_session_watches(state, watcher, watch.sessions)
      put_in(state.watches[watcher], %{jobs: jobs, sessions: sessions})
    end)
  end

  # A job watch fires once, on the first poll that sees the job terminal --
  # including a job that was already terminal when the watch was placed
  # (subscribing to a finished job answers immediately rather than never).
  defp poll_job_watches(state, watcher, jobs) do
    Enum.reduce(jobs, {state, jobs}, fn id, {state, jobs} ->
      case safe(fn -> ActionLog.job(id) end) do
        %{status: status} = job when status != :running ->
          {enqueue(state, watcher, %{kind: :watch, row: watch_row(job)}), MapSet.delete(jobs, id)}

        _running_or_unavailable ->
          {state, jobs}
      end
    end)
  end

  defp poll_session_watches(state, watcher, sessions) do
    Enum.reduce(sessions, {state, sessions}, fn {sid, watermark}, {state, sessions} ->
      fresh =
        safe(fn -> ActionLog.recent_jobs(sid, 50) end)
        |> List.wrap()
        |> Enum.filter(fn job ->
          job.status != :running and job.finished_at != nil and job.finished_at > watermark
        end)

      state =
        Enum.reduce(fresh, state, fn job, state ->
          enqueue(state, watcher, %{kind: :watch, row: watch_row(job)})
        end)

      watermark = Enum.reduce(fresh, watermark, &max(&1.finished_at, &2))
      {state, Map.put(sessions, sid, watermark)}
    end)
  end

  defp watch_row(job) do
    %{
      job_id: job.id,
      intent: job.intent,
      status: job.status,
      elapsed_ms: job.elapsed_ms,
      result: job.result
    }
  end

  defp safe(fun) do
    fun.()
  catch
    :exit, _reason -> nil
  end

  defp now_iso, do: DateTime.to_iso8601(DateTime.utc_now())

  # -- delivery ----------------------------------------------------------------

  # Replay every unacked outbox row as one digest so a reconnecting session
  # sees "while you were away: ..." rather than nothing (#3839). Scoped to
  # this instance's session, so a transport connecting here never drains a
  # sibling instance's notifications out of the shared database.
  defp replay_unacked(transports, session_id) do
    case unacked_rows(session_id) do
      [] ->
        :ok

      rows ->
        lines = Enum.map_join(rows, "\n", &("- " <> line_of(&1)))
        content = "while you were away, #{length(rows)} job(s) finished:\n" <> lines

        meta = %{
          "source" => "jobs",
          "replay" => Integer.to_string(length(rows)),
          "severity" => severity(rows)
        }

        deliver(transports, "notifications/claude/channel", %{
          "content" => content,
          "meta" => meta
        })

        rows |> Enum.map(& &1.id) |> ack_all()
    end
  end

  defp unacked_rows(session_id) do
    ActionLog.unacked_outbox(session_id)
  catch
    :exit, _reason -> []
  end

  # The ledger interactions must never crash the notifier: its state is the
  # live transport registry, and losing it silences every future
  # notification until the transports reconnect (#3874). A row whose ack
  # was lost stays unacked and may replay as a duplicate later -- accepted.
  defp ack_all(ids) do
    ActionLog.ack_outbox(ids)
  catch
    :exit, _reason -> 0
  end

  defp deliver(transports, method, params) do
    message = %{"jsonrpc" => "2.0", "method" => method, "params" => params}
    Enum.each(transports, fn pid -> send(pid, {:mcp_send, message}) end)
  end

  # -- rendering ---------------------------------------------------------------

  # One finish is one line -- the output stays behind Jobs.output (#3934);
  # only a failure carries its reason, because the reason IS the news.
  defp render_entries([%{kind: kind, row: row}]) do
    status = Atom.to_string(row.status)

    content =
      case row.status do
        :done -> line_of(row)
        _failure -> line_of(row) <> "\n" <> String.slice(row.result || "", 0, 500)
      end

    meta = %{
      "source" => "jobs",
      "job" => row.job_id,
      "status" => status,
      "severity" => severity([row])
    }

    {content, if(kind == :watch, do: Map.put(meta, "watch", "1"), else: meta)}
  end

  defp render_entries(entries) do
    rows = Enum.map(entries, & &1.row)
    lines = Enum.map_join(rows, "\n", &("- " <> line_of(&1)))
    content = "#{length(rows)} job(s) finished:\n" <> lines

    {content,
     %{
       "source" => "jobs",
       "digest" => Integer.to_string(length(rows)),
       "severity" => severity(rows)
     }}
  end

  defp line_of(row) do
    status = Atom.to_string(row.status)

    line =
      "job #{row.job_id} (#{row.intent || "no intent"}): #{status} in #{elapsed_s(row.elapsed_ms)}s"

    case row.status do
      :done -> line
      _failure -> line <> " -- " <> reason_of(row.result)
    end
  end

  defp reason_of(nil), do: "no result recorded"

  defp reason_of(result) do
    result |> String.split("\n", parts: 2) |> hd() |> String.slice(0, 120)
  end

  defp severity(rows) do
    if Enum.all?(rows, &(&1.status == :done)), do: "info", else: "failure"
  end

  defp elapsed_s(nil), do: 0.0
  defp elapsed_s(ms), do: ms / 1000
end
