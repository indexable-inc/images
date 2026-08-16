defmodule IxMcp.Fleet.WatchOffer do
  @moduledoc """
  The elicitation path to the human: when a session connects while warnings
  are STANDING, ask the user directly whether to watch for changes.

  Elicitation (`elicitation/create`, via `IxMcp.Ask`) is the one MCP request
  that reaches the person through the client's own UI rather than through
  the model's context, so the opt-in decision is genuinely the human's: no
  agent judgment call, no relayed suggestion. A yes starts the same
  singleton `WarningsWatch` an agent would have started, so the dedup story
  is unchanged.

  Bounded on purpose, because an unasked dialog is a cost:
    * only when something is standing at connect (an all-green fleet asks
      nothing; the passive snapshot line already names the affordance);
    * only once per kernel boot, however many sessions connect;
    * only when nobody is already watching;
    * best-effort: a client that cannot elicit gets no dialog and no error.
  """

  alias FleetMesh.Engine
  alias IxMcp.Ask
  alias IxMcp.Fleet.WarningsWatch

  @offered_key {__MODULE__, :offered}

  @doc """
  Offer the watch if this boot has not offered it yet. Returns immediately;
  the dialog (if any) runs in its own task so the transport loop never
  blocks on a human.
  """
  @spec maybe_offer(keyword()) :: :ok
  def maybe_offer(opts \\ []) do
    if :persistent_term.get(@offered_key, false) do
      :ok
    else
      :persistent_term.put(@offered_key, true)
      {:ok, _pid} = Task.start(fn -> offer(opts) end)
      :ok
    end
  end

  @doc false
  # Test hook: a persistent term survives between tests by design.
  @spec reset() :: :ok
  def reset, do: :persistent_term.put(@offered_key, false)

  defp offer(opts) do
    done = Keyword.get(opts, :done, fn -> :ok end)

    try do
      run_offer(opts)
    rescue
      # A client without elicitation answers with a JSON-RPC error that Ask
      # raises on. The offer is an affordance, not a requirement.
      _error -> :ok
    end

    done.()
  end

  defp run_offer(opts) do
    snapshot = Keyword.get(opts, :snapshot, &Engine.snapshot/0)
    ask = Keyword.get(opts, :ask, &Ask.user/2)
    start_watch = Keyword.get(opts, :start_watch, &WarningsWatch.start/1)

    standing = for {id, %{state: state}} <- snapshot.(), state != :green, do: id

    if standing != [] and WarningsWatch.watcher() == nil do
      question =
        "Fleet warnings standing: #{Enum.join(Enum.sort(standing), ", ")}. " <>
          "Watch for changes and notify this kernel's sessions on any transition?"

      case ask.(question,
             options: [
               {"yes", "watch: one channel line per transition"},
               {"no", "just this snapshot"}
             ]
           ) do
        {:ok, "yes"} -> start_watch.("elicited at connect")
        _other -> :ok
      end
    end
  end
end
