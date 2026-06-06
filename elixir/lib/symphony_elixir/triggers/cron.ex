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
  - if `last_fired_at` is set, we compute `next_fire_after(last_fired_at)`
    via `CronExpression`. If that moment is <= now, we start one run,
    set `last_fired_at = now`, and skip any intermediate missed windows.
    This is the `systemd Persistent=true` semantic: at most one catch-up
    fire per workflow per restart, not N firings for N missed monthly slots.

  ## Trigger context

  The started run's trigger map carries:

      %{
        kind: :cron,
        schedule: "@monthly",            # the workflow's declared schedule
        timezone: "UTC",                 # currently always UTC; reserved
        scheduled_for: "2026-06-01T00:00:00Z",  # ISO 8601 string, the cron-matching moment we caught up to
        fired_at: "2026-06-01T00:00:14Z",       # ISO 8601 string, actual wall clock when we started
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
  require Logger

  alias SymphonyElixir.{Config, CronExpression, CronState, WorkflowCatalog}
  alias SymphonyElixir.Runtime.Ingress

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
  def init(_opts) do
    poll_ms = Config.get().cron_poll_ms
    schedule_tick(poll_ms)
    {:ok, %{poll_ms: poll_ms}}
  end

  @impl true
  def handle_info(:tick, state) do
    tick_once()
    schedule_tick(state.poll_ms)
    {:noreply, state}
  end

  @impl true
  def handle_call(:poll_now, _from, state) do
    tick_once()
    {:reply, :ok, state}
  end

  defp schedule_tick(ms), do: Process.send_after(self(), :tick, ms)

  defp tick_once do
    now = DateTime.utc_now()

    WorkflowCatalog.for_trigger_kind(:cron)
    |> Enum.each(fn entry -> evaluate_workflow(entry, now) end)
  end

  defp evaluate_workflow(entry, now) do
    case CronExpression.parse(entry.trigger.schedule) do
      {:ok, parsed} ->
        case CronState.get_last_fired(entry.name) do
          nil ->
            # First time we observe this workflow; do NOT fire on boot. Seed
            # the watermark so the first fire happens at the next match.
            :ok = CronState.seed_if_unset(entry.name, now)

          %DateTime{} = last_fired ->
            case CronExpression.next_fire_after(parsed, last_fired) do
              {:ok, next} ->
                if DateTime.compare(next, now) != :gt do
                  fire(entry, next, now)
                end

              {:error, reason} ->
                Logger.warning("Cron next_fire_after failed for workflow=#{entry.name} schedule=#{entry.trigger.schedule}: #{inspect(reason)}")
            end
        end

      {:error, reason} ->
        Logger.warning("Cron schedule unparseable for workflow=#{entry.name} schedule=#{inspect(entry.trigger.schedule)}: #{inspect(reason)}")
    end
  end

  defp fire(entry, %DateTime{} = scheduled_for, %DateTime{} = now) do
    trigger = %{
      kind: :cron,
      schedule: entry.trigger.schedule,
      timezone: entry.trigger.timezone,
      scheduled_for: DateTime.to_iso8601(scheduled_for),
      fired_at: DateTime.to_iso8601(now),
      input: entry.trigger.input
    }

    case Ingress.start_by_trigger(trigger) do
      {:ok, started} ->
        :ok = CronState.record_fire(entry.name, now)

        Logger.info("Cron started runs=#{Enum.map_join(started, ",", & &1.run_id)} workflow=#{entry.name} scheduled_for=#{DateTime.to_iso8601(scheduled_for)}")

      {:error, reason} ->
        Logger.warning("Cron failed to start workflow=#{entry.name}: #{inspect(reason)}")
    end
  end
end
