defmodule IxMcp.Agents.Events do
  @moduledoc """
  The lead-side ledger for `IxMcp.Agents`: per-child normalized events,
  session refs for resume, final results, and the board-ready graph.

  Also the one consumer of the lead's harness mailbox: a linked drain task
  blocks in `AgentHarness.wait_for_message/3` and turns `:final`/`:error`
  messages into stored results, `await/2` replies, and `agent_finished`
  MCP notifications (the PrWatch producer pattern). State is in-memory:
  a crash of this server drops the ledger but never the children, which
  live under the harness supervisor.
  """

  use GenServer

  alias IxMcp.MCP.Notifier

  @events_cap 200
  @notify_result_cap 2_000

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

  @spec events(String.t()) :: [map()]
  def events(id), do: GenServer.call(__MODULE__, {:events, id})

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
      meta: %{},
      events: %{},
      sessions: %{},
      finals: %{},
      waiters: %{}
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
    {:noreply, %{state | meta: Map.put(state.meta, id, meta)}}
  end

  def handle_cast({:record, id, kind, data}, state) do
    event = Map.merge(data, %{kind: kind, at_ms: System.system_time(:millisecond)})
    events = Map.update(state.events, id, [event], &Enum.take([event | &1], @events_cap))
    {:noreply, %{state | events: events}}
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

  defp settle(state, id, result) do
    {waiters, rest} = Map.pop(state.waiters, id, [])
    Enum.each(waiters, &GenServer.reply(&1, result))
    notify_final(id, result, Map.get(state.meta, id))
    %{state | finals: Map.put(state.finals, id, result), waiters: rest}
  end

  defp notify_final(id, result, meta) do
    {status, text} =
      case result do
        {:ok, text} -> {"done", text}
        {:error, reason} -> {"error", inspect_text(reason)}
      end

    Notifier.notify("notifications/message", %{
      "level" => if(status == "done", do: "info", else: "error"),
      "logger" => "ix_mcp.agents",
      "data" => %{
        "event" => "agent_finished",
        "agent" => id,
        "status" => status,
        "backend" => meta && Atom.to_string(meta.backend),
        "result" => String.slice(text, 0, @notify_result_cap)
      }
    })
  end

  defp notify_message(state, msg) do
    Notifier.notify("notifications/message", %{
      "level" => "info",
      "logger" => "ix_mcp.agents",
      "data" => %{
        "event" => "agent_message",
        "agent" => msg.from,
        "text" => String.slice(msg.text, 0, @notify_result_cap)
      }
    })

    state
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
