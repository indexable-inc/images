defmodule IxMcp.Fleet.Watch do
  @moduledoc """
  Polls the fleet for the conditions in `IxMcp.Fleet.Alerts` and pushes each
  genuinely new one into the connected session.

  ## Why the channel and not `notifications/message`

  MCP's logging notification is the obvious transport and it does not work
  here. Claude Code receives `notifications/message` and never surfaces it
  (#3785, already recorded in `IxMcp.MCP.Notifier`), so an alert sent that way
  is delivered, acked, and invisible -- a guard that cannot fire, which is the
  exact failure this whole module exists to avoid. Fleet alerts therefore ride
  `notifications/claude/channel`, the same transport job finishes already use,
  paired with the `experimental.claude/channel` capability the server declares
  at initialize.

  `logging/setLevel` is still honoured, as the coarse threshold: it is what
  the MCP specification gives a client to say "stop telling me the small
  stuff", the server already advertises the `logging` capability, and level
  filtering is cheap to respect even when the delivery method differs. See
  `IxMcp.MCP.Server`.

  ## Unsubscribing

  Two granularities, because level alone cannot express "mute this one thing":

  * `logging/setLevel` raises the floor for every fleet alert at once.
  * `Fleet.mute/2` silences one predicate by id, durably.

  The mute is durable on purpose (ENG-11209). A mute held in memory un-mutes
  on the next reconnect, which means the operator who muted a spamming alert
  gets it back a few minutes later and concludes muting does not work.

  ## Deduplication

  A condition that keeps being true is not news twice. Each hit carries a
  fingerprint identifying the condition *instance*, and `ActionLog` decides
  newness with one guarded INSERT -- so a fault standing for a week announces
  once, while a genuinely new burst announces again. `Fleet.alerts/0` reads
  the standing set; `Fleet.forget/1` re-arms it.
  """

  use GenServer

  alias IxMcp.ActionLog
  alias IxMcp.Fleet.Alerts
  alias IxMcp.Fleet.ClickHouse
  alias IxMcp.MCP.Notifier

  # Fleet faults are minutes-scale, and every poll is an ssh round trip to the
  # leader. Five minutes keeps the load invisible and still catches a fault
  # inside one coffee break.
  @poll_ms Application.compile_env(:ix_mcp, :fleet_poll_ms, 300_000)

  # Nothing is polled until the fleet has had a moment to exist: at boot the
  # kernel is usually mid-startup and an ssh attempt during it just produces a
  # spurious observability_blind.
  @initial_delay_ms Application.compile_env(:ix_mcp, :fleet_initial_delay_ms, 30_000)

  @spec start_link(term()) :: GenServer.on_start()
  def start_link(opts) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @doc """
  Poll now and return what was announced, rather than waiting for the timer.
  Used by `Fleet.check/0` so an operator can ask "what is wrong right now"
  and by the tests, which must not sleep for a poll interval.
  """
  @spec poll_now() :: %{announced: [map()], suppressed: non_neg_integer(), errors: [String.t()]}
  def poll_now, do: GenServer.call(__MODULE__, :poll_now, 120_000)

  @impl true
  def init(_opts) do
    Process.send_after(self(), :poll, @initial_delay_ms)
    {:ok, %{level: "warning"}}
  end

  @doc """
  Set the minimum level delivered, from `logging/setLevel`. Session state per
  the specification, so deliberately NOT durable -- unlike a mute, which is.
  """
  @spec set_level(String.t()) :: :ok
  def set_level(level) when is_binary(level) do
    GenServer.cast(__MODULE__, {:set_level, level})
  end

  @doc "The current minimum level."
  @spec level() :: String.t()
  def level, do: GenServer.call(__MODULE__, :level)

  @impl true
  def handle_cast({:set_level, level}, state), do: {:noreply, %{state | level: level}}

  @impl true
  def handle_call(:level, _from, state), do: {:reply, state.level, state}

  def handle_call(:poll_now, _from, state) do
    {:reply, run_poll(state.level), state}
  end

  @impl true
  def handle_info(:poll, state) do
    run_poll(state.level)
    Process.send_after(self(), :poll, @poll_ms)
    {:noreply, state}
  end

  def handle_info(_message, state), do: {:noreply, state}

  # -- polling -----------------------------------------------------------------

  @doc """
  One poll cycle, with its two collaborators passed in rather than reached for:
  the ClickHouse read and the ledger that decides newness.

  This is public because it is what the tests drive. The alternative -- reading
  the real fleet from a test -- would make the mute and dedup behaviour
  unverifiable except on a day the fleet happens to be broken, and a guard
  nobody has watched fail is not a guard.
  """
  @spec run_poll(String.t(), keyword()) :: %{
          announced: [map()],
          suppressed: non_neg_integer(),
          errors: [String.t()]
        }
  def run_poll(threshold, opts \\ []) do
    query_fun = Keyword.get(opts, :query_fun, &ClickHouse.query/1)
    log = Keyword.get(opts, :action_log, ActionLog)
    notify = Keyword.get(opts, :notify, &announce/1)

    muted = Enum.map(ActionLog.fleet_mutes(log), & &1.id)
    outcomes = Alerts.evaluate(muted, query_fun)

    errors = for {_id, {:error, reason}} <- outcomes, do: reason

    hits =
      outcomes
      |> Enum.flat_map(fn
        {_id, {:ok, hits}} -> hits
        {_id, {:error, _reason}} -> []
      end)
      |> Enum.filter(&at_or_above?(&1.level, threshold))

    # Newness is decided for every hit, including the ones the level filter
    # will drop, so raising the threshold cannot cause a later lowering to
    # replay a backlog of faults the operator already lived through.
    {fresh, suppressed} =
      Enum.split_with(
        hits,
        &ActionLog.fleet_alert_new?(&1.fingerprint, &1.predicate, &1.summary, log)
      )

    if fresh != [], do: notify.(fresh)

    %{announced: fresh, suppressed: length(suppressed), errors: errors}
  end

  defp announce(hits) do
    content =
      case hits do
        [hit] ->
          "fleet alert (#{hit.predicate}): #{hit.summary}"

        many ->
          "#{length(many)} fleet alerts:\n" <> Enum.map_join(many, "\n", &("- " <> &1.summary))
      end

    Notifier.channel(content <> "\n\n" <> mute_hint(hits), %{
      "source" => "fleet",
      "severity" => "failure",
      "predicates" => hits |> Enum.map(& &1.predicate) |> Enum.uniq() |> Enum.join(",")
    })
  end

  # The way out travels with the thing being complained about. An alert that
  # does not say how to stop it is one the reader can only endure.
  defp mute_hint(hits) do
    ids = hits |> Enum.map(& &1.predicate) |> Enum.uniq()

    "Too noisy? Fleet.mute(#{inspect(hd(ids))}) silences this one durably; Fleet.alerts() lists what is standing."
  end

  # RFC 5424 order, as MCP's logging levels use it: lower index is less
  # severe, so a hit is delivered when its level is at or above the floor.
  @levels ~w(debug info notice warning error critical alert emergency)

  @doc false
  @spec at_or_above?(String.t(), String.t()) :: boolean()
  def at_or_above?(level, threshold) do
    with hit when hit != nil <- Enum.find_index(@levels, &(&1 == level)),
         floor when floor != nil <- Enum.find_index(@levels, &(&1 == threshold)) do
      hit >= floor
    else
      # An unknown level must not silently swallow an alert.
      _ -> true
    end
  end

  @doc "Every level name `logging/setLevel` accepts."
  @spec levels() :: [String.t()]
  def levels, do: @levels
end
