defmodule SymphonyElixir.Triggers.Cron do
  @moduledoc """
  Time-based trigger. Ticks every `Config.cron_poll_ms` (default 60s),
  walks every `.sym` workflow with `trigger.kind = :cron`, and starts one
  IR run per workflow when a scheduled fire moment has passed since the
  last recorded fire.

  ## Fire semantics

  Per workflow we persist `last_fired_at` via `SymphonyElixir.CronState`,
  keyed by workflow name. On each tick:

  - if the workflow has no `last_fired_at` yet, we SEED it to the current
    moment without firing. This is the boot-time "do not catch up just
    because we deployed" behaviour. A brand new cron workflow fires for the
    first time at its next scheduled match after deployment.
  - if `last_fired_at` is set, we compute the next scheduled moment after
    it via `next_due/4`. If that moment is <= now, we start one run,
    set `last_fired_at = now`, and skip any intermediate missed windows.
    This is the `systemd Persistent=true` semantic: at most one catch-up
    fire per workflow per restart, not N firings for N missed monthly slots.

  ## Time zones

  Schedules are evaluated against wall-clock time in the workflow's
  declared `tz "..."` zone (`entry.trigger.timezone`, default `"UTC"`).
  `CronExpression` stays pure wall-clock math over naive datetimes; this
  module shifts the watermark into the zone, finds the next wall-clock
  match, and converts it back to an absolute instant for the <= now
  comparison. So `0 9 * * *` with `tz "America/Los_Angeles"` fires at 9am
  LA time year-round instead of drifting an hour across DST.

  DST edges are resolved when the wall-clock match maps back to absolute
  time:

  - spring-forward gap (e.g. 02:30 does not exist): fire at the first
    valid instant after the gap (the transition moment, e.g. 03:00).
    Skipping instead would silently drop a daily job once a year;
    firing at the boundary keeps "runs every day" true and is what
    cronds like systemd and Vixie cron converge on.
  - fall-back repeat (e.g. 02:30 happens twice): fire at the first
    (earlier) occurrence only. The watermark then sits past that wall
    time, so the second occurrence cannot re-match: one fire, not two.

  An unknown zone is a loud logged skip, exactly like an unparseable
  schedule. There is no silent fall back to UTC.

  ## Trigger context

  The started run's trigger map carries:

      %{
        kind: :cron,
        schedule: "@monthly",            # the workflow's declared schedule
        timezone: "America/Los_Angeles", # the workflow's declared zone (default "UTC")
        scheduled_for: "2026-06-01T00:00:00-07:00",  # ISO 8601 string with zone offset, the cron-matching moment we caught up to
        fired_at: "2026-06-01T07:00:14Z",            # ISO 8601 string, actual wall clock when we started
        input: %{...}                    # whatever the workflow's `input` block says
      }

  Datetimes are serialized as ISO 8601 strings because the trigger
  round-trips through JSON via `IR.Store`, and `Jason` has no built-in
  encoder for `DateTime`. Callers that need the datetime back parse it
  with `DateTime.from_iso8601/1`. The `schedule` field is load-bearing for
  resolution: `start_by_trigger/2` re-selects the workflow whose declared
  cron schedule equals it, so the tick fires exactly the workflow it
  evaluated.

  ## Dedupe

  Ingress is unconditional; dedupe is by `last_fired_at`. Two ticks racing
  for the same calendar minute is not a real risk because
  `CronState.record_fire/2` is serialized through its GenServer, and the
  next tick reads the updated value through ETS. If `record_fire` fails
  we log and skip; the next tick will see the old `last_fired_at` and
  retry.
  """

  use GenServer

  alias SymphonyElixir.Config
  alias SymphonyElixir.CronExpression
  alias SymphonyElixir.CronState
  alias SymphonyElixir.Runtime.Ingress
  alias SymphonyElixir.WorkflowCatalog

  require Logger

  @doc """
  `opts` may carry `:run_opts`, forwarded to `Ingress.start_by_trigger/2`
  (`:engine`, `:store_opts`), so a test drives a full tick against a fake
  engine and an isolated store. Production passes nothing.
  """
  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @doc """
  Manually trigger one poll cycle. Test hook; production fires via the
  internal :tick message.
  """
  @spec poll_now() :: :ok
  def poll_now do
    GenServer.call(__MODULE__, :poll_now)
  end

  @impl true
  def init(opts) do
    poll_ms = Config.get().cron_poll_ms
    schedule_tick(poll_ms)
    {:ok, %{poll_ms: poll_ms, run_opts: Keyword.get(opts, :run_opts, [])}}
  end

  @impl true
  def handle_info(:tick, state) do
    tick_once(state.run_opts)
    schedule_tick(state.poll_ms)
    {:noreply, state}
  end

  @impl true
  def handle_call(:poll_now, _from, state) do
    tick_once(state.run_opts)
    {:reply, :ok, state}
  end

  defp schedule_tick(ms), do: Process.send_after(self(), :tick, ms)

  defp tick_once(run_opts) do
    now = DateTime.utc_now()

    :cron
    |> WorkflowCatalog.for_trigger_kind()
    |> Enum.each(fn entry -> evaluate_workflow(entry, now, run_opts) end)
  end

  defp evaluate_workflow(entry, now, run_opts) do
    case CronExpression.parse(entry.trigger.schedule) do
      {:ok, parsed} ->
        case CronState.get_last_fired(entry.name) do
          nil ->
            # First time we observe this workflow; do NOT fire on boot. Seed
            # the watermark so the first fire happens at the next match.
            :ok = CronState.seed_if_unset(entry.name, now)

          %DateTime{} = last_fired ->
            maybe_fire_due(entry, parsed, last_fired, now, run_opts)
        end

      {:error, reason} ->
        Logger.warning("Cron schedule unparseable for workflow=#{entry.name} schedule=#{inspect(entry.trigger.schedule)}: #{inspect(reason)}")
    end
  end

  defp maybe_fire_due(entry, parsed, last_fired, now, run_opts) do
    case next_due(parsed, last_fired, now, entry.trigger.timezone) do
      {:fire, scheduled_for} ->
        fire(entry, scheduled_for, now, run_opts)

      :not_due ->
        :ok

      {:error, :time_zone_not_found} ->
        Logger.warning("Cron timezone unknown for workflow=#{entry.name} timezone=#{inspect(entry.trigger.timezone)}; skipping (no UTC fallback)")

      {:error, reason} ->
        Logger.warning("Cron next_due failed for workflow=#{entry.name} schedule=#{entry.trigger.schedule} timezone=#{entry.trigger.timezone}: #{inspect(reason)}")
    end
  end

  @doc """
  Pure decision function for one workflow tick: given the parsed schedule,
  the `last_fired_at` watermark, `now` (both absolute instants), and the
  workflow's IANA zone, returns

  - `{:fire, scheduled_for}` when a scheduled wall-clock moment in the
    zone falls after the watermark and at or before `now`; `scheduled_for`
    is the absolute instant expressed in the zone (its offset survives
    ISO 8601 serialization).
  - `:not_due` when the next match is still in the future.
  - `{:error, :time_zone_not_found}` for a zone the IANA database does not
    know; callers must skip loudly, never assume UTC.
  """
  @spec next_due(CronExpression.t(), DateTime.t(), DateTime.t(), String.t()) ::
          {:fire, DateTime.t()} | :not_due | {:error, term()}
  def next_due(parsed, %DateTime{} = last_fired, %DateTime{} = now, timezone) when is_binary(timezone) do
    with {:ok, last_local} <- DateTime.shift_zone(last_fired, timezone),
         {:ok, wall_next} <- CronExpression.next_fire_after(parsed, DateTime.to_naive(last_local)),
         {:ok, scheduled_for} <- wall_to_absolute(wall_next, timezone) do
      if DateTime.after?(scheduled_for, now) do
        :not_due
      else
        {:fire, scheduled_for}
      end
    end
  end

  # Maps a wall-clock match back to an absolute instant, resolving the two
  # DST edges (see the moduledoc for why each choice):
  # gap -> first valid instant after it; ambiguity -> earlier occurrence.
  defp wall_to_absolute(%NaiveDateTime{} = wall, timezone) do
    case DateTime.from_naive(wall, timezone) do
      {:ok, %DateTime{} = dt} -> {:ok, dt}
      {:gap, _just_before, %DateTime{} = just_after} -> {:ok, just_after}
      {:ambiguous, %DateTime{} = first, _second} -> {:ok, first}
      {:error, reason} -> {:error, reason}
    end
  end

  defp fire(entry, %DateTime{} = scheduled_for, %DateTime{} = now, run_opts) do
    trigger = %{
      kind: :cron,
      schedule: entry.trigger.schedule,
      timezone: entry.trigger.timezone,
      scheduled_for: DateTime.to_iso8601(scheduled_for),
      fired_at: DateTime.to_iso8601(now),
      input: entry.trigger.input
    }

    case Ingress.start_by_trigger(trigger, run_opts) do
      {:ok, started} ->
        :ok = CronState.record_fire(entry.name, now)

        Logger.info("Cron started runs=#{Enum.map_join(started, ",", & &1.run_id)} workflow=#{entry.name} scheduled_for=#{DateTime.to_iso8601(scheduled_for)}")

      {:error, reason} ->
        Logger.warning("Cron failed to start workflow=#{entry.name}: #{inspect(reason)}")
    end
  end
end
