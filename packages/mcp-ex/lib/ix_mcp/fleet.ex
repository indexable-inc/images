defmodule IxMcp.Fleet do
  @moduledoc """
  Run Elixir on other BEAM nodes over the tailnet -- one specific node, a
  random one, or a fan-out across many -- and the kernel's frontend for the
  fleet's warning machinery (alerts, digest, heartbeat, mutes).

  The mesh client itself lives in `FleetMesh.Mesh` (packages/fleet-mesh),
  the single copy of the `IX_BEAM_NODES` / `IX_BEAM_COOKIE` deploy contract;
  this module delegates to it so `Fleet.exec(...)` stays the surface agents
  already type.

      Fleet.exec(:"beamd@host-a.tailnet.ts.net", fn -> System.schedulers_online() end)
      Fleet.exec_any("node() |> to_string()")            # a random node
      Fleet.multicall(Fleet.nodes(), fn -> node() end, timeout: 10_000)

  When to reach for this: expensive work of any kind -- compiling, builds,
  test suites, large data crunching (fan out with `multicall/3`) -- plus
  linux-only behavior checks and work wanting many cores or hosts; nodes are
  root on the fleet. Stay local for anything touching this workstation's
  files, repos, or bindings, small low-latency evals, stateful work
  (bindings do not persist remotely), and darwin-specific behavior.

  Strings versus funs for remote code: see `FleetMesh.Mesh`. Funs keep
  compile-time checking but need a byte-identical `erl_eval` on both ends
  (probed per node, instructive error on divergence); strings sidestep
  module resolution entirely.
  """

  alias IxMcp.ActionLog
  alias IxMcp.Fleet.Digest
  alias IxMcp.Fleet.Topology
  alias IxMcp.Fleet.Watch

  alias FleetMesh.Mesh
  alias IxMcp.Fleet.WarningsWatch

  # The mesh client is FleetMesh.Mesh; these delegates keep the REPL surface
  # (`Fleet.exec` and friends) that sessions and docs already use, while the
  # implementation lives once, shared with the test-ide dashboard.
  @doc "The configured fleet nodes; see `FleetMesh.Mesh.nodes/0`."
  defdelegate nodes(), to: Mesh

  @doc "Start distribution lazily; see `FleetMesh.Mesh.ensure_dist/0`."
  defdelegate ensure_dist(), to: Mesh

  @doc "Run code on one node; see `FleetMesh.Mesh.exec/3`."
  defdelegate exec(target, code, opts \\ []), to: Mesh

  @doc "Run code on a random configured node; see `FleetMesh.Mesh.exec_any/2`."
  defdelegate exec_any(code, opts \\ []), to: Mesh

  @doc "Run code on the least-loaded node; see `FleetMesh.Mesh.exec_least_loaded/2`."
  defdelegate exec_least_loaded(code, opts \\ []), to: Mesh

  @doc "Fan code out across nodes; see `FleetMesh.Mesh.multicall/3`."
  defdelegate multicall(targets, code, opts \\ []), to: Mesh

  @doc """
  Which hosts the BEAM is actually on, probed now: reachable, unreachable, or
  unknown-because-distribution-is-down. This is the same summary the server
  puts in its `initialize` instructions.
  """
  @spec topology() :: Topology.t()
  def topology, do: Topology.summary()

  @doc """
  Poll the fleet for alert conditions right now and say what happened.

  Returns `%{announced: hits, suppressed: n, errors: reasons}`. A non-empty
  `errors` means part of the fleet could not be read -- which is NOT the same
  as a healthy fleet, and is why it is a separate key rather than an empty
  `announced`.
  """
  @spec check() :: %{announced: [map()], suppressed: non_neg_integer(), errors: [String.t()]}
  def check, do: Watch.poll_now()

  @doc """
  Silence one alert predicate durably. Survives reconnects and restarts.
  Valid ids come from `Alerts.ids/0`; an unknown id is refused
  rather than accepted into a mute list that then does nothing.
  """
  @spec mute(String.t(), String.t() | nil) :: :ok | {:error, String.t()}
  def mute(predicate, reason \\ nil) when is_binary(predicate) do
    if predicate in mutable() do
      case ActionLog.mute_fleet_predicate(predicate, reason) do
        :disabled ->
          {:error,
           "the action log is degraded, so the mute was not stored and " <>
             "#{predicate} will keep announcing"}

        _stored ->
          :ok
      end
    else
      known = Enum.join(mutable(), ", ")
      {:error, "unknown predicate #{inspect(predicate)}; known: #{known}"}
    end
  end

  @doc """
  Everything that can be muted.

  * a discrete predicate id -- "this specific alarm is wrong"
  * `"heartbeat"` -- stop the hourly baseline line
  * `"anomaly"` -- stop the immediate out-of-band line
  * `"digest"` -- both of the above
  * `"digest:warning"` and friends -- keep the line, drop one category from it

  Five shapes rather than one, because they answer genuinely different asks and
  an operator who can only mute everything will mute everything.
  """
  @spec mutable() :: [String.t()]
  def mutable do
    policy_ids() ++
      ["observability_blind", "digest", "heartbeat", "anomaly"] ++
      for(level <- ~w(warning error crit alert emerg), do: "digest:" <> level)
  end

  # observability_blind is not in the policy: a failed read surfaces as
  # :unknown on the condition it broke and Watch synthesizes one blindness
  # hit from those. It is mutable all the same, so it is named here.
  defp policy_ids do
    Enum.map(FleetMesh.Policy.configured!().conditions(), &Atom.to_string(&1.id))
  end

  @doc """
  Un-silence one alert predicate. `{:error, _}` when the log is degraded and
  the change could not be stored.
  """
  @spec unmute(String.t()) :: :ok | {:error, String.t()}
  def unmute(predicate) when is_binary(predicate) do
    case ActionLog.unmute_fleet_predicate(predicate) do
      :disabled -> {:error, "the action log is degraded, so the unmute was not stored"}
      _ok -> :ok
    end
  end

  @doc """
  What is currently muted, and what alerts are standing. A condition that
  fired once and is still true lives here rather than being re-announced, so
  this is where to look when the channel has gone quiet and you want to know
  whether that means "fine" or "already told you".
  """
  @spec alerts() :: %{muted: [map()], standing: [map()], level: String.t()}
  def alerts do
    %{
      muted: ActionLog.fleet_mutes(),
      standing: ActionLog.fleet_alerts_seen(),
      level: Watch.level()
    }
  end

  @doc """
  Expand the last heartbeat window into what was actually counted: the top
  host, level and unit combinations, with a sample message each.

  The line is a pointer, not the content. Without this, getting curious means
  writing ClickHouse by hand at precisely the moment attention is available.
  """
  @spec digest() :: {:ok, [map()]} | {:error, String.t()}
  def digest, do: expand(Watch.digest_state())

  defp expand(%{last: nil}),
    do: {:error, "no heartbeat has been sent yet; Fleet.heartbeat_now() builds one"}

  defp expand(%{last: %{from: from, to: to}}), do: Digest.detail(from, to)

  @doc "Send a heartbeat immediately. `{:ok, nil}` if the window was empty."
  @spec heartbeat_now() :: {:ok, map() | nil} | {:error, String.t()}
  def heartbeat_now, do: Watch.heartbeat_now()

  @doc """
  Check the last complete minute for an anomaly immediately. `{:ok, nil}` means
  in band, which is the answer roughly 99.5% of the time by construction.
  """
  @spec anomaly_now() :: {:ok, map() | nil} | {:error, String.t()}
  def anomaly_now, do: Watch.anomaly_now()

  @doc """
  Read or set the heartbeat period in seconds (default 3600, minimum 60).

  Hourly rather than per-minute for a measured reason: 87.1% of minutes are
  non-empty, so a 60s heartbeat costs roughly **1,250 lines a day** while an
  hourly one costs 24. Anomalies do not wait for the hour -- they emit within
  a minute of detection, at a measured 10.3 a day.
  """
  @spec heartbeat_period(pos_integer() | nil) :: pos_integer() | :ok | {:error, String.t()}
  def heartbeat_period(seconds \\ nil)
  def heartbeat_period(nil), do: Watch.digest_state().period_s
  def heartbeat_period(seconds) when is_integer(seconds), do: Watch.set_heartbeat_period(seconds)

  @doc """
  The anomaly threshold in force for the current clock hour, and the quantile
  it is taken at. Useful for answering "why did that not fire".
  """
  @spec anomaly_threshold() :: map() | nil
  def anomaly_threshold, do: Watch.digest_state().threshold

  @doc """
  Forget standing alerts so they announce again: `:all`, or one predicate id.
  Use after fixing something, to confirm the fix by silence rather than by
  assumption.
  """
  @spec forget(:all | String.t()) :: integer()
  def forget(scope \\ :all), do: ActionLog.forget_fleet_alerts(scope)

  @doc """
  The warning conditions' current states: `%{id => %{state, since, detail}}`
  where state is `:green | :red | :unknown`. `%{}` means the first
  evaluation has not finished (or no catalog is loaded). The same picture
  every session gets in its connect instructions.
  """
  @spec warnings() :: FleetMesh.Engine.states()
  def warnings, do: FleetMesh.Engine.snapshot()

  @doc """
  Opt this kernel in to warning EDGE notifications: one channel line per
  transition (green -> red, red -> green, either -> unknown), on top of the
  snapshot every session already gets on connect.

  **Usually call this only when the human explicitly asks.** Every session
  on this kernel shares one channel, one watcher covers everyone, and the
  singleton makes a second watcher impossible: if someone already watches,
  this returns `{:already_watching, who}` and changes nothing. `who` for
  your own call comes from `requested_by`, so name yourself.
  """
  @spec watch_warnings(String.t()) :: :ok | {:already_watching, String.t() | nil}
  def watch_warnings(requested_by \\ "unnamed session") do
    case WarningsWatch.start(requested_by) do
      {:ok, _pid} ->
        :ok

      {:error, {:already_started, _pid}} ->
        {:already_watching, WarningsWatch.watcher()}
    end
  end

  @doc "Stop the warning edge watch. `:ok` even when nothing was watching."
  @spec unwatch_warnings() :: :ok
  def unwatch_warnings, do: WarningsWatch.stop()
end
