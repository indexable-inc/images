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

  ## What the tests cannot see

  The predicates are SQL, and every test here stubs the query function. That
  makes the query text the one layer nothing looks at, and it is exactly where
  the worst defect in this module has already hidden once: an over-broad device
  exclusion left `kernel_storage` matching zero rows in thirty days while its
  own documented rate claimed one hit a month. **A layer that every test mocks
  is a layer with no tests.** Two tests in `fleet_alerts_test.exs` therefore
  assert on the SQL text itself rather than on values fed around it, and any
  new predicate should get the same treatment.

  ## Deduplication

  A condition that keeps being true is not news twice. Each hit carries a
  fingerprint identifying the condition *instance*, and `ActionLog` decides
  newness with one guarded INSERT -- so a fault standing for a week announces
  once, while a genuinely new burst announces again. `Fleet.alerts/0` reads
  the standing set; `Fleet.forget/1` re-arms it.
  """

  use GenServer

  alias FleetMesh.ClickHouse
  alias FleetMesh.Condition
  alias FleetMesh.Policy
  alias IxMcp.ActionLog
  alias IxMcp.Fleet.Digest
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
  # Above the sum of the three predicate reads' own timeouts (3 x 60s in
  # ClickHouse), or a slow leader makes Fleet.check/0 raise while the poll it
  # is waiting on is still making progress.
  def poll_now, do: GenServer.call(__MODULE__, :poll_now, 200_000)

  # Heartbeat period: the visible baseline, at 24 lines a day. Hourly rather
  # than per-minute because the measured cost of per-minute is ~1,250 lines a
  # day (87.1% of minutes are non-empty), which is not wallpaper but what
  # wallpaper becomes when there is too much of it.
  @default_heartbeat_s Application.compile_env(:ix_mcp, :fleet_heartbeat_period_s, 3_600)

  # Anomaly detection runs once a minute, aligned to the wall clock by
  # next_minute_ms/0. Measurement is per-minute because that is the granularity
  # the distribution supports; only emission is rare, and it is rare by
  # construction rather than by hope -- see IxMcp.Fleet.Digest on why the
  # threshold is a quantile and not a ratio.

  @doc """
  Send a heartbeat now. `{:ok, nil}` means the window was empty and nothing
  was said.
  """
  @spec heartbeat_now() :: {:ok, map() | nil} | {:error, String.t()}
  def heartbeat_now, do: GenServer.call(__MODULE__, :heartbeat_now, 120_000)

  @doc """
  Check the last complete minute for an anomaly now. `{:ok, nil}` means it was
  in band, which is the overwhelmingly common answer.
  """
  @spec anomaly_now() :: {:ok, map() | nil} | {:error, String.t()}
  def anomaly_now, do: GenServer.call(__MODULE__, :anomaly_now, 120_000)

  @doc """
  Change the heartbeat period, in seconds. Minimum 60s -- the measurement says
  a per-minute heartbeat costs ~1,250 lines a day, so anything shorter is
  re-creating the problem this cadence exists to solve.
  """
  @spec set_heartbeat_period(pos_integer()) :: :ok | {:error, String.t()}
  def set_heartbeat_period(seconds) when is_integer(seconds) do
    if seconds >= 60 do
      GenServer.cast(__MODULE__, {:heartbeat_period, seconds})
    else
      {:error,
       "heartbeat period must be at least 60s (a 60s heartbeat is already ~1,250 lines/day), got #{seconds}"}
    end
  end

  @doc "Heartbeat period, the last window summarised, and the cached threshold."
  @spec digest_state() :: %{period_s: pos_integer(), last: map() | nil, threshold: map() | nil}
  def digest_state, do: GenServer.call(__MODULE__, :digest_state)

  # Whether the timers are armed. The process always starts -- callers need the
  # level and heartbeat state, and the tests drive run_poll/2, run_heartbeat/2
  # and run_anomaly/2 directly -- but polling is off in the test environment and
  # in any sandboxed build, because a poll is an ssh to a production host. A
  # test suite that reaches out to hil-compute-2 is wrong whether or not it
  # passes, and in a nix build it would fail with no network and read as a fleet
  # outage.
  @poll_enabled Application.compile_env(:ix_mcp, :fleet_watch_enabled, true)

  @impl true
  def init(_opts) do
    if @poll_enabled do
      Process.send_after(self(), :poll, @initial_delay_ms)
      Process.send_after(self(), :heartbeat, @initial_delay_ms + 5_000)
      Process.send_after(self(), :anomaly, @initial_delay_ms + 10_000)
    end

    {:ok,
     %{
       level: "warning",
       heartbeat_s: @default_heartbeat_s,
       last_digest: nil,
       # {clock_hour, value}: recomputing the threshold is a 7-day scan, which
       # is fine hourly and not fine every 60 seconds.
       threshold: nil
     }}
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

  def handle_cast({:heartbeat_period, seconds}, state),
    do: {:noreply, %{state | heartbeat_s: seconds}}

  @impl true
  def handle_call(:level, _from, state), do: {:reply, state.level, state}

  def handle_call(:poll_now, _from, state) do
    {:reply, run_poll(state.level), state}
  end

  def handle_call(:digest_state, _from, state) do
    threshold =
      case state.threshold do
        {hour, value} -> %{hour: hour, value: value, quantile: Digest.anomaly_quantile()}
        nil -> nil
      end

    {:reply, %{period_s: state.heartbeat_s, last: state.last_digest, threshold: threshold}, state}
  end

  def handle_call(:heartbeat_now, _from, state) do
    case run_heartbeat(state.heartbeat_s) do
      {:ok, nil} -> {:reply, {:ok, nil}, state}
      {:ok, digest} -> {:reply, {:ok, digest}, %{state | last_digest: digest}}
      {:error, reason} -> {:reply, {:error, reason}, state}
    end
  end

  def handle_call(:anomaly_now, _from, state) do
    {reply, state} = anomaly_cycle(state)
    {:reply, reply, state}
  end

  @impl true
  def handle_info(:poll, state) do
    run_poll(state.level)
    Process.send_after(self(), :poll, @poll_ms)
    {:noreply, state}
  end

  def handle_info(:heartbeat, state) do
    state =
      case run_heartbeat(state.heartbeat_s) do
        {:ok, nil} -> state
        {:ok, digest} -> %{state | last_digest: digest}
        # A window that could not be read is announced by the alert path's
        # observability_blind predicate, not a second time from here.
        {:error, _reason} -> state
      end

    Process.send_after(self(), :heartbeat, state.heartbeat_s * 1_000)
    {:noreply, state}
  end

  def handle_info(:anomaly, state) do
    {_reply, state} = anomaly_cycle(state)
    # To the wall-clock boundary, not `now + 60s`. Rescheduling after the cycle
    # makes the true period 60s plus query latency (measured ~450ms over ssh),
    # which drifts a whole minute every ~2 hours and silently skips it.
    Process.send_after(self(), :anomaly, next_minute_ms())
    {:noreply, state}
  end

  def handle_info(_message, state), do: {:noreply, state}

  defp next_minute_ms do
    60_000 - rem(System.system_time(:millisecond), 60_000) + 2_000
  end

  # -- heartbeat and anomaly ---------------------------------------------------

  @doc """
  Build one heartbeat and announce it unless muted. Public for the same reason
  `run_poll/2` is: suppression and mute behaviour has to be testable without a
  broken fleet to hand.
  """
  @spec run_heartbeat(pos_integer(), keyword()) :: {:ok, map() | nil} | {:error, String.t()}
  def run_heartbeat(period_s, opts \\ []) do
    query_fun = Keyword.get(opts, :query_fun, &ClickHouse.query/1)
    log = Keyword.get(opts, :action_log, ActionLog)
    notify = Keyword.get(opts, :notify, &announce_heartbeat/1)

    muted = Enum.map(ActionLog.fleet_mutes(log), & &1.id)

    if muted?(muted, "heartbeat") do
      {:ok, nil}
    else
      with {:ok, digest} when digest != nil <- Digest.build(period_s, query_fun) do
        emit(drop_muted_levels(digest, muted), notify)
      end
    end
  end

  @doc """
  Check the last complete minute against the cached per-hour threshold and
  announce it if out of band. `cached` is `{clock_hour, value}` or nil; the
  refreshed value comes back so the caller can hold it.
  """
  @spec run_anomaly(term(), keyword()) :: {{:ok, map() | nil} | {:error, String.t()}, term()}
  def run_anomaly(cached, opts \\ []) do
    query_fun = Keyword.get(opts, :query_fun, &ClickHouse.query/1)
    log = Keyword.get(opts, :action_log, ActionLog)
    notify = Keyword.get(opts, :notify, &announce_anomaly/1)
    hour = Keyword.get(opts, :hour, Digest.measured_hour())

    muted = Enum.map(ActionLog.fleet_mutes(log), & &1.id)

    if muted?(muted, "anomaly") do
      {{:ok, nil}, cached}
    else
      with_threshold(cached, hour, query_fun, notify)
    end
  end

  # The threshold is a 7-day scan, so it is computed once per clock hour and
  # reused for that hour's sixty checks.
  defp with_threshold({hour, value}, hour, query_fun, notify),
    do: {detect(value, query_fun, notify), {hour, value}}

  defp with_threshold(_stale, hour, query_fun, notify) do
    case Digest.threshold(hour, query_fun) do
      {:ok, value} -> {detect(value, query_fun, notify), {hour, value}}
      # A threshold that could not be read is dropped rather than kept stale:
      # judging this hour against last hour's number is worse than not judging.
      {:error, reason} -> {{:error, reason}, nil}
    end
  end

  defp detect(threshold, query_fun, notify) do
    case Digest.check_anomaly(threshold, query_fun) do
      {:ok, nil} ->
        {:ok, nil}

      {:ok, anomaly} ->
        notify.(anomaly)
        {:ok, anomaly}

      {:error, reason} ->
        {:error, reason}
    end
  end

  # "digest" mutes both rates, because that is what somebody typing it means;
  # "heartbeat" and "anomaly" mute one each.
  defp muted?(muted, id), do: id in muted or "digest" in muted

  # Dropping every counted level leaves nothing to say, and a line reading
  # "0 notable" is the thing mutes exist to stop.
  defp emit(%{total: 0}, _notify), do: {:ok, nil}

  defp emit(digest, notify) do
    notify.(digest)
    {:ok, digest}
  end

  # Per-category mute: "digest:warning" removes warnings from the count rather
  # than silencing the whole line.
  defp drop_muted_levels(digest, muted) do
    dropped =
      for "digest:" <> level <- muted, level != "", into: MapSet.new() do
        if level == "warning", do: "warn", else: level
      end

    counts = Map.reject(digest.counts, fn {level, _n} -> MapSet.member?(dropped, level) end)
    %{digest | counts: counts, total: counts |> Map.values() |> Enum.sum()}
  end

  # The heartbeat is meant to become furniture -- that is what makes an anomaly
  # legible -- so it is one quiet line, severity "info", with no hint text.
  defp announce_heartbeat(digest) do
    Notifier.channel("fleet: " <> Digest.render(digest), %{
      "source" => "fleet_heartbeat",
      "severity" => "info",
      "total" => Integer.to_string(digest.total)
    })
  end

  # The anomaly is the line somebody actually reads, so it carries the culprit
  # hosts, the drill-in and the way out.
  defp announce_anomaly(anomaly) do
    Notifier.channel(
      "fleet ANOMALY: " <>
        Digest.render_anomaly(anomaly) <>
        "\nFleet.digest() expands the window; Fleet.mute(\"anomaly\") stops these.",
      %{
        "source" => "fleet_anomaly",
        "severity" => "failure",
        "count" => Integer.to_string(anomaly.count)
      }
    )
  end

  defp anomaly_cycle(state) do
    {reply, threshold} = run_anomaly(state.threshold)
    {reply, %{state | threshold: threshold}}
  end

  # -- polling -----------------------------------------------------------------

  @doc """
  One poll cycle, with its two collaborators passed in rather than reached
  for: the condition catalog (built by the configured `FleetMesh.Policy`,
  its reads embedded) and the ledger that decides newness.

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
    log = Keyword.get(opts, :action_log, ActionLog)
    notify = Keyword.get(opts, :notify, &announce/1)

    deliverable = Keyword.get(opts, :deliverable, deliverable?())

    muted = Enum.map(ActionLog.fleet_mutes(log), & &1.id)

    conditions =
      Keyword.get_lazy(opts, :conditions, fn -> Policy.configured!().conditions() end)

    outcomes = evaluate(conditions, muted)

    errors = for {_id, {:error, reason}} <- outcomes, do: reason

    hits =
      Enum.flat_map(outcomes, fn
        {_id, {:ok, hits}} -> hits
        {_id, {:error, _reason}} -> []
      end)

    dispose(hits, threshold, log, notify, errors, deliverable)
  end

  @typedoc """
  What should happen to one hit, and the distinction three separate bugs came
  from collapsing.

  `fleet_alert_new?/4` is a query that is also a commit: it answers "is this
  new?" and records "this has been announced" in the same call. So "was not
  announced" has to be split, because two of these three consume the
  fingerprint and one must not:

    * `:deliver` -- goes to a transport. Consumes.
    * `:consume` -- deliberately dropped by the level floor. Consumes, because
      raising the floor means "stop telling me about these" and lowering it
      later must not dump the backlog.
    * `:defer`   -- could not be attempted at all: no transport attached, or a
      degraded ledger. Must NOT consume. Nothing happened, so nothing is
      recorded, and the next poll sees the fault fresh.

  Collapsing `:consume` and `:defer` is what produced the level-filter
  ordering bug, the fingerprint written with no listener, and the degraded log
  answering "already seen" to everything. They were one defect in three places.
  """
  @type disposition :: :deliver | :consume | :defer

  # A blind fingerprint has no time bucket, so without this a second outage is
  # silent forever: the heartbeat and anomaly paths deliberately swallow their
  # own errors on the grounds that observability_blind will announce them, and
  # if it cannot, an outage becomes indistinguishable from a healthy hour.
  defp rearm_blind(errors, log) do
    if errors == [], do: ActionLog.forget_fleet_alerts("observability_blind", log)
  end

  # Every hit routes through one classification rather than three guards in
  # three places. The table in the disposition typedoc is the specification;
  # this is it, executable.
  defp dispose(_hits, _threshold, _log, _notify, errors, false = _deliverable) do
    # Nothing can be attempted, so nothing is classified and nothing is
    # recorded. The fleet path has no outbox replay the way jobs do (#3839),
    # so a fingerprint written with no listener buries the fault forever.
    %{announced: [], suppressed: 0, errors: errors}
  end

  defp dispose(hits, threshold, log, notify, errors, true = _deliverable) do
    rearm_blind(errors, log)
    # Newness is asked for EVERY hit, before the level floor is applied, so
    # that raising the floor and later lowering it cannot replay a backlog the
    # operator already lived through. Filtering first is the ordering bug this
    # replaced, and the comment claiming otherwise sat directly above it.
    classified =
      Enum.map(hits, fn hit ->
        if ActionLog.fleet_alert_new?(hit.fingerprint, hit.predicate, hit.summary, log) do
          {classify(hit, threshold), hit}
        else
          # Already announced on an earlier poll: still true, not still news.
          {:consume, hit}
        end
      end)

    announced = for {:deliver, hit} <- classified, do: hit
    if announced != [], do: notify.(announced)

    %{
      announced: announced,
      suppressed: Enum.count(classified, &match?({:consume, _}, &1)),
      errors: errors
    }
  end

  @blind_id "observability_blind"

  # The bridge from condition states to the outcome shape the dispose
  # pipeline speaks. A red condition's detail is its hits (the policy shapes
  # them); anything else red returns is a policy defect and reads as a failed
  # read rather than as silence.
  @spec evaluate([Condition.t()], [String.t()]) ::
          %{String.t() => {:ok, [map()]} | {:error, String.t()}}
  defp evaluate(conditions, muted) do
    reads =
      conditions
      |> Enum.reject(&(Atom.to_string(&1.id) in muted))
      |> Map.new(fn condition ->
        id = Atom.to_string(condition.id)
        {id, outcome(id, Condition.evaluate(condition))}
      end)

    if @blind_id in muted, do: reads, else: Map.put(reads, @blind_id, blindness(reads))
  end

  defp outcome(_id, {:green, _detail}), do: {:ok, []}
  defp outcome(_id, {:red, hits}) when is_list(hits), do: {:ok, hits}

  defp outcome(id, {:red, other}),
    do: {:error, "condition #{id} returned non-list hits: #{inspect(other)}"}

  defp outcome(_id, {:unknown, reason}) when is_binary(reason), do: {:error, reason}
  defp outcome(_id, {:unknown, reason}), do: {:error, inspect(reason)}

  # One hit naming every condition that could not be read, keyed on the
  # reason so a persistent outage announces once rather than every poll.
  # Mechanism, not policy: "the read failed" carries no fleet fact, which is
  # why this synthesis lives here and not in the private catalog.
  defp blindness(reads) do
    case for({id, {:error, reason}} <- reads, do: {id, reason}) do
      [] ->
        {:ok, []}

      failures ->
        reason = failures |> Enum.map(&elem(&1, 1)) |> Enum.uniq() |> Enum.join("; ")
        which = failures |> Enum.map(&elem(&1, 0)) |> Enum.sort() |> Enum.join(", ")

        {:ok,
         [
           %{
             predicate: @blind_id,
             # A warning, not critical: not being able to see the fleet is
             # serious, but it is usually a laptop off the tailnet rather
             # than an outage, and shouting critical for that is how the
             # level gets raised past the things that matter.
             level: "warning",
             fingerprint: @blind_id <> ":" <> blind_hash(reason),
             summary:
               "cannot read fleet telemetry (#{which}) -- this is NOT a report of a healthy fleet: #{reason}"
           }
         ]}
    end
  end

  defp blind_hash(text) do
    :crypto.hash(:sha256, text) |> Base.encode16(case: :lower) |> String.slice(0, 12)
  end

  # "I cannot see the fleet" ignores the level floor. It is a `warning` so that
  # ordinary noise-reduction does not read it as an outage, but the natural
  # response to noise -- raise the floor to `error` -- must not be the thing
  # that hides blindness.
  @spec classify(map(), String.t()) :: disposition()
  defp classify(%{predicate: "observability_blind"}, _threshold), do: :deliver

  defp classify(hit, threshold) do
    if at_or_above?(hit.level, threshold), do: :deliver, else: :consume
  end

  # Whether any transport is attached to receive a channel event. Tests inject
  # `deliverable: true` because they assert on the return value rather than on
  # the wire.
  defp deliverable?, do: Notifier.transports() > 0

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
