defmodule IxMcp.Agents.Events do
  @moduledoc """
  The lead-side ledger for `IxMcp.Agents`: per-child normalized events,
  session refs for resume, final results, and the board-ready graph.

  Also the one consumer of the lead's harness mailbox: a linked drain task
  blocks in `AgentHarness.wait_for_message/3` and turns `:final`/`:error`
  messages into stored results, `await/2` replies, and durable
  `agent_finished` announcements.

  A final goes out the way a job finish does (#3839, #3934): an outbox row
  first, then `IxMcp.MCP.Notifier.publish/1`. That is the whole point of the
  indirection -- a direct channel push is dropped when no transport is
  attached, and a child's final is unrecoverable once dropped, since the CLI
  process that produced it is gone. The durable row survives the gap and
  replays on reconnect. A live push remains only as the degraded-ledger
  fallback, where there is no row to replay and silence would be worse.

  State is in-memory: a crash of this server drops the ledger but never the
  children, which live under the harness supervisor. The finals map is
  therefore a cache of what was announced, never the announcement itself.
  """

  use GenServer

  alias IxMcp.ActionLog
  alias IxMcp.MCP.Notifier
  alias IxMcp.Session

  @events_cap 200
  @notify_result_cap 2_000

  # How often a working child's directory row is re-stamped. Events arrive
  # per parsed stream line, far too often to write SQLite each time; 5s
  # keeps the row comfortably inside the directory's 30s liveness window
  # (`IxMcp.Sessions` @fresh_within_s) at a fifth of the writes a
  # per-tick stamp would cost.
  @beat_every_ms 5_000

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @spec register_spawn(String.t(), map()) :: :ok
  def register_spawn(id, meta), do: GenServer.cast(__MODULE__, {:register, id, meta})

  @doc "Record one normalized child event (called by the runner per parsed line)."
  @spec record(String.t(), atom(), map()) :: :ok
  def record(id, kind, data \\ %{}) do
    GenServer.cast(__MODULE__, {:record, id, kind, data})
  end

  @spec put_session(String.t(), String.t()) :: :ok
  def put_session(id, ref), do: GenServer.cast(__MODULE__, {:session, id, ref})

  @doc "The child's CLI session/thread ref, for resume on wake. Nil on first run."
  @spec session_ref(String.t()) :: String.t() | nil
  def session_ref(id), do: GenServer.call(__MODULE__, {:session_ref, id})

  @doc """
  One child's recent normalized events, NEWEST FIRST, capped at the most
  recent #{@events_cap}. Each record prepends, so index 0 is the latest line
  the child produced and the oldest is what falls off at the cap.
  """
  @spec events(String.t()) :: [map()]
  def events(id), do: GenServer.call(__MODULE__, {:events, id})

  @doc """
  Declare that the child owes a new final, dropping the stored one so a
  following `await/2` blocks for the next turn. `IxMcp.Agents.send/2` calls
  this BEFORE handing the message to the harness, and it is a call rather
  than a cast for that reason: were the clear to land after the child had
  already produced its post-send final, it would delete the new answer and
  the await would block for a turn that had already happened.
  """
  @spec expect_turn(String.t()) :: :ok
  def expect_turn(id), do: GenServer.call(__MODULE__, {:expect_turn, id})

  @spec finals() :: %{String.t() => {:ok, String.t()} | {:error, term()}}
  def finals, do: GenServer.call(__MODULE__, :finals)

  @spec await(String.t(), timeout()) :: {:ok, String.t()} | {:error, term()}
  def await(id, timeout \\ :infinity) do
    GenServer.call(__MODULE__, {:await, id}, timeout)
  catch
    :exit, {:timeout, _call} -> {:error, :timeout}
  end

  @spec graph() :: %{nodes: [map()], edges: [[String.t()]]}
  def graph, do: GenServer.call(__MODULE__, :graph)

  # -- server --

  @impl true
  def init(opts) do
    state = %{
      harness: Keyword.fetch!(opts, :harness),
      action_log: Keyword.get(opts, :action_log, ActionLog),
      meta: %{},
      events: %{},
      sessions: %{},
      finals: %{},
      waiters: %{},
      beats: %{}
    }

    {:ok, state, {:continue, :drain}}
  end

  @impl true
  def handle_continue(:drain, state) do
    server = self()
    harness = state.harness
    {:ok, _pid} = Task.start_link(fn -> drain_lead(harness, server) end)
    {:noreply, state}
  end

  # Blocks forever on the lead mailbox. `:not_found` means the harness is
  # (re)starting -- one_for_all resets take a moment -- so back off and
  # retry rather than spinning; the loop self-heals when the lead returns.
  defp drain_lead(harness, server) do
    case AgentHarness.wait_for_message(harness, AgentHarness.lead_id(), :infinity) do
      {:ok, msgs} -> Enum.each(msgs, &GenServer.cast(server, {:lead_message, &1}))
      _ -> Process.sleep(1_000)
    end

    drain_lead(harness, server)
  end

  @impl true
  def handle_cast({:register, id, meta}, state) do
    # Stamped at registration so the child's dot exists from the spawn,
    # not from its first output line. `started_ms` is stamped here too: it is
    # the only moment the lead knows the child's clock started, and a final
    # with no elapsed time reads as an instant finish.
    meta = Map.put_new(meta, :started_ms, System.system_time(:millisecond))
    {:noreply, beat(%{state | meta: Map.put(state.meta, id, meta)}, id)}
  end

  def handle_cast({:record, id, kind, data}, state) do
    event = Map.merge(data, %{kind: kind, at_ms: System.system_time(:millisecond)})
    events = Map.update(state.events, id, [event], &Enum.take([event | &1], @events_cap))
    {:noreply, beat(%{state | events: events}, id)}
  end

  def handle_cast({:session, id, ref}, state) do
    {:noreply, %{state | sessions: Map.put(state.sessions, id, ref)}}
  end

  def handle_cast({:lead_message, msg}, state) do
    case msg.kind do
      :final -> {:noreply, settle(state, msg.from, {:ok, msg.text})}
      :error -> {:noreply, settle(state, msg.from, {:error, msg.text})}
      :message -> {:noreply, notify_message(state, msg)}
    end
  end

  @impl true
  # Drop the stored final so the next `await/2` blocks for the turn the
  # message will produce. Returning the previous turn's final to a caller who
  # just steered the child is a wrong answer delivered in 0.0s, which is the
  # worst shape a wrong answer can take.
  def handle_call({:expect_turn, id}, _from, state) do
    {:reply, :ok, %{state | finals: Map.delete(state.finals, id)}}
  end

  def handle_call({:session_ref, id}, _from, state) do
    {:reply, Map.get(state.sessions, id), state}
  end

  def handle_call({:events, id}, _from, state) do
    {:reply, Map.get(state.events, id, []), state}
  end

  def handle_call(:finals, _from, state), do: {:reply, state.finals, state}

  def handle_call({:await, id}, from, state) do
    case Map.get(state.finals, id) do
      nil -> {:noreply, %{state | waiters: Map.update(state.waiters, id, [from], &[from | &1])}}
      result -> {:reply, result, state}
    end
  end

  def handle_call(:graph, _from, state) do
    statuses = AgentHarness.subagent_status(state.harness)

    nodes =
      [%{"id" => "lead", "label" => "lead (kernel session)", "state" => "now"}] ++
        Enum.map(Enum.sort(statuses), fn {id, status} ->
          %{
            "id" => id,
            "label" => node_label(id, Map.get(state.meta, id)),
            "state" => node_state(status, Map.get(state.finals, id))
          }
        end)

    edges = statuses |> Map.keys() |> Enum.sort() |> Enum.map(&["lead", &1])
    {:reply, %{nodes: nodes, edges: edges}, state}
  end

  # Announce BEFORE replying to waiters: `IxMcp.Agents.await/2` acks the row
  # its reply just carried, and an ack that runs before the row exists finds
  # nothing to suppress -- the finish would then be announced again moments
  # after the caller already had it in hand.
  defp settle(state, id, result) do
    announce_final(state, id, result)
    {waiters, rest} = Map.pop(state.waiters, id, [])
    Enum.each(waiters, &GenServer.reply(&1, result))
    %{state | finals: Map.put(state.finals, id, result), waiters: rest}
  end

  defp announce_final(state, id, result) do
    {status, text} = outcome(result)
    backend = backend_of(state, id)
    opts = [session_id: lead_session(), intent: backend, elapsed_ms: elapsed_ms(state, id)]

    case durable_announce(id, status, text, opts, state.action_log) do
      {:notify, outbox} -> Notifier.publish(outbox)
      :disabled -> push_live(id, status, text, backend)
    end
  end

  defp durable_announce(id, status, text, opts, log) do
    ActionLog.announce_agent(id, status, text, opts, log)
  catch
    # A dead ledger reads the same as a disabled one: no row was written, so
    # the caller must fall back rather than assume the announcement is safe.
    :exit, _reason -> :disabled
  end

  # The pre-durable path, kept for exactly one case: no outbox row exists to
  # replay, so a live push is the only surface left and beats the silence.
  # Delivery is best-effort by construction -- with no transport attached
  # this is dropped, which is the failure the durable row removes.
  defp push_live(id, status, text, backend) do
    attrs = %{
      "source" => "agents",
      "event" => "agent_finished",
      "agent" => id,
      "status" => Atom.to_string(status),
      "severity" => severity(status)
    }

    attrs = if backend, do: Map.put(attrs, "backend", backend), else: attrs

    Notifier.channel(
      "agent #{id} finished: #{status}\n#{String.slice(text || "", 0, @notify_result_cap)}",
      attrs
    )
  end

  defp outcome({:ok, text}), do: {:done, text}
  defp outcome({:error, reason}), do: {:failed, inspect_text(reason)}

  # A child that failed is not an "info" event. The old direct push left
  # severity to the Notifier's `info` default, so a dead child announced
  # itself as quietly as a clean one.
  defp severity(:done), do: "info"
  defp severity(_failed), do: "failure"

  # The lead's own session: the one listening for this child. Nil only when
  # the session Agent is unreachable, which costs the row its reconnect
  # replay scope but not its existence.
  defp lead_session do
    Session.ids().session_id
  catch
    :exit, _reason -> nil
  end

  defp backend_of(state, id) do
    case Map.get(state.meta, id) do
      %{backend: backend} -> Atom.to_string(backend)
      _unregistered -> nil
    end
  end

  defp elapsed_ms(state, id) do
    case Map.get(state.meta, id) do
      %{started_ms: started} -> System.system_time(:millisecond) - started
      _unregistered -> nil
    end
  end

  # Deliberately NOT durable, unlike a final. A mid-run message is the child
  # narrating; replaying it after a reconnect would recite stale chatter, and
  # losing one costs nothing recoverable -- the child is still running and its
  # final still carries the outcome. The no-silent-death invariant is about
  # outcomes, and this is not one.
  defp notify_message(state, msg) do
    Notifier.channel(
      String.slice(msg.text, 0, @notify_result_cap),
      %{"source" => "agents", "event" => "agent_message", "agent" => msg.from}
    )

    state
  end

  # Re-stamp the child's session-directory heartbeat (ENG-12004), rate
  # limited to @beat_every_ms. A finished child stops producing events, so
  # its row goes stale and its dot dies inside the directory's 30s window
  # with no terminal write needed. Best-effort like the registration:
  # readers lose a heartbeat, the child loses nothing.
  defp beat(state, id) do
    now = System.system_time(:millisecond)

    with %{child_session: session} when is_integer(session) <- Map.get(state.meta, id),
         last when now - last >= @beat_every_ms <- Map.get(state.beats, id, 0) do
      try do
        ActionLog.heartbeat_session(session, state.action_log)
      catch
        :exit, _ -> :ok
      end

      %{state | beats: Map.put(state.beats, id, now)}
    else
      _ -> state
    end
  end

  defp inspect_text(reason) when is_binary(reason), do: reason
  defp inspect_text(reason), do: inspect(reason)

  defp node_label(id, nil), do: id
  defp node_label(id, meta), do: "#{id} [#{meta.backend}]"

  defp node_state(:working, _final), do: "now"
  defp node_state(_status, {:error, _reason}), do: "blocked"
  defp node_state(_status, {:ok, _text}), do: "done"
  defp node_state(:idle, nil), do: "next"
  defp node_state(:terminated, nil), do: "blocked"
end
