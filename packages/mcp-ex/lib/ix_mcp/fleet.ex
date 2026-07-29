defmodule IxMcp.Fleet do
  @moduledoc """
  Run Elixir on other BEAM nodes over the tailnet -- one specific node, a
  random one, or a fan-out across many -- using native Erlang distribution
  and `:erpc`.

  This module is deliberately **topology-agnostic**: it never hard-codes any
  hostname. The node list and the shared cookie come from the environment,
  injected by whatever deploys this server:

    * `IX_BEAM_NODES` -- comma/space/newline separated node names, e.g.
      `beamd@host-a.tailnet.ts.net beamd@host-b.tailnet.ts.net`. Empty/unset
      means no fleet is configured and the fan-out helpers return
      `{:error, :no_nodes}`.
    * `IX_BEAM_COOKIE` (or `IX_BEAM_COOKIE_FILE` pointing at a file) -- the
      shared distribution cookie. Every node in the fleet, and this server,
      must agree on it; it is a root-equivalent credential.
    * `IX_BEAM_LOCAL_NAME` -- optional. The long name this node registers as
      when distribution starts. Defaults to `mcp-ex@<hostname>`.

  Distribution is started **lazily** on the first fleet call (see
  `ensure_dist/0`): the release ships with distribution off so its sandboxed
  build and tests never try to open an epmd listen socket.

  Code ships to remote nodes as a **source string** evaluated with
  `Code.eval_string/1` or as a **zero-arity fun** run via `:erpc`. Funs keep
  compile-time checking and need no escaping, but they resolve against the
  remote node's code: a fun calling a module defined only in this workspace
  fails there with `:undef`. Strings sidestep module resolution entirely, so
  use them when the code leans on locally defined modules.

  An interpreted fun also carries the md5 of the local `erl_eval` bytecode
  and the remote node raises `badfun` unless its `erl_eval` is byte-identical.
  Both ends of this fleet are nix-controlled and expected to share the erlang
  pin, so that md5 matching is the supported contract; fun dispatch verifies
  it per node up front (an MFA `:erpc` probe, itself version-safe, cached in
  `:persistent_term`) and returns `{:error, {:erl_eval_mismatch, ...}}` with
  update instructions instead of letting `badfun` escape mid-task.

      Fleet.exec(:"beamd@host-a.tailnet.ts.net", fn -> System.schedulers_online() end)
      Fleet.exec_any("node() |> to_string()")            # a random node
      Fleet.multicall(Fleet.nodes(), fn -> node() end, timeout: 10_000)

  When to reach for this: expensive work of any kind -- compiling, builds,
  test suites, large data crunching (fan out with `multicall/3`) -- plus
  linux-only behavior checks and work wanting many cores or hosts; nodes are
  root on the fleet. Stay local for anything
  touching this workstation's files, repos, or bindings, small low-latency
  evals, stateful work (bindings do not persist remotely), and darwin-specific
  behavior.
  """

  alias IxMcp.ActionLog
  alias IxMcp.Fleet.Alerts
  alias IxMcp.Fleet.Topology
  alias IxMcp.Fleet.Watch

  @nodes_env "IX_BEAM_NODES"
  @cookie_env "IX_BEAM_COOKIE"
  @cookie_file_env "IX_BEAM_COOKIE_FILE"
  @local_name_env "IX_BEAM_LOCAL_NAME"

  @default_timeout 15_000

  @type exec_result :: {:ok, term()} | {:error, term()}

  @typedoc "Remote work: Elixir source, or a zero-arity fun (see moduledoc caveat)."
  @type code :: String.t() | (-> term())

  defguardp is_code(code) when is_binary(code) or is_function(code, 0)

  @doc """
  The configured fleet nodes, parsed from `IX_BEAM_NODES`. Returns `[]` when
  the variable is unset or empty.
  """
  @spec nodes() :: [node()]
  def nodes do
    (System.get_env(@nodes_env) || "")
    |> String.split([",", " ", "\n", "\t"], trim: true)
    # Operator-set deploy config, a bounded list; :erpc requires node atoms.
    # astlog-ignore: no-unsafe-to-atom
    |> Enum.map(&String.to_atom/1)
  end

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
    if predicate in Alerts.ids() do
      case ActionLog.mute_fleet_predicate(predicate, reason) do
        :disabled ->
          {:error,
           "the action log is degraded, so the mute was not stored and " <>
             "#{predicate} will keep announcing"}

        _stored ->
          :ok
      end
    else
      known = Enum.join(Alerts.ids(), ", ")
      {:error, "unknown predicate #{inspect(predicate)}; known: #{known}"}
    end
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
  Forget standing alerts so they announce again: `:all`, or one predicate id.
  Use after fixing something, to confirm the fix by silence rather than by
  assumption.
  """
  @spec forget(:all | String.t()) :: integer()
  def forget(scope \\ :all), do: ActionLog.forget_fleet_alerts(scope)

  @doc """
  Ensure distributed Erlang is running on this node, starting it (and setting
  the shared cookie) on first use. Idempotent.
  """
  @spec ensure_dist() :: {:ok, node()} | {:error, term()}
  def ensure_dist do
    if Node.alive?() do
      {:ok, node()}
    else
      start_distribution()
    end
  end

  @doc """
  Run `code` (source string or zero-arity fun) on `target` and return
  `{:ok, value}` or `{:error, reason}`.
  A dead or unreachable node fails only this call. `:timeout` (ms) defaults to
  #{@default_timeout}.
  """
  @spec exec(node(), code(), keyword()) :: exec_result()
  def exec(target, code, opts \\ []) when is_atom(target) and is_code(code) do
    timeout = Keyword.get(opts, :timeout, @default_timeout)

    with {:ok, _} <- ensure_dist(),
         :ok <- code_compat(target, code, timeout) do
      try do
        {:ok, remote_call(target, code, timeout)}
      catch
        kind, reason -> {:error, {kind, reason}}
      end
    end
  end

  @doc """
  Evaluate `code` on one random configured node. `{:error, :no_nodes}` when
  `IX_BEAM_NODES` is empty.
  """
  @spec exec_any(code(), keyword()) :: exec_result()
  def exec_any(code, opts \\ []) when is_code(code) do
    case nodes() do
      [] -> {:error, :no_nodes}
      ns -> exec(Enum.random(ns), code, opts)
    end
  end

  @doc """
  Evaluate `code` on the least-loaded reachable node (by scheduler run-queue
  length). `{:error, :no_nodes}` when none are configured.
  """
  @spec exec_least_loaded(code(), keyword()) :: exec_result()
  def exec_least_loaded(code, opts \\ []) when is_code(code) do
    case nodes() do
      [] ->
        {:error, :no_nodes}

      ns ->
        with {:ok, _} <- ensure_dist(),
             {:ok, target} <- least_loaded(ns, Keyword.get(opts, :probe_timeout, 5_000)) do
          exec(target, code, opts)
        end
    end
  end

  @doc """
  Fan `code` out across `targets`, returning `{node, {:ok, value} | {:error,
  reason}}` per node. One dead node cannot poison the batch. `:timeout` (ms)
  is enforced per node.
  """
  @spec multicall([node()], code(), keyword()) :: [{node(), exec_result()}]
  def multicall(targets, code, opts \\ []) when is_list(targets) and is_code(code) do
    timeout = Keyword.get(opts, :timeout, @default_timeout)

    case ensure_dist() do
      {:ok, _} ->
        targets
        |> remote_multicall(code, timeout)
        |> Enum.zip(targets)
        |> Enum.map(fn {result, target} -> {target, normalize(result)} end)

      {:error, reason} ->
        Enum.map(targets, &{&1, {:error, reason}})
    end
  end

  # -- internals ------------------------------------------------------------

  @spec remote_call(node(), code(), timeout()) :: term()
  defp remote_call(target, code, timeout) when is_binary(code) do
    {value, _binding} = :erpc.call(target, Code, :eval_string, [code], timeout)
    value
  end

  defp remote_call(target, fun, timeout) when is_function(fun, 0) do
    :erpc.call(target, fun, timeout)
  end

  # Strings need no compatibility: the remote node evaluates them with its
  # own erl_eval. Funs do (see moduledoc); probe before dispatch.
  @spec code_compat(node(), code(), timeout()) :: :ok | {:error, term()}
  defp code_compat(_target, code, _timeout) when is_binary(code), do: :ok
  defp code_compat(target, fun, timeout) when is_function(fun, 0), do: fun_compat(target, timeout)

  # Probed once per node with a plain MFA call (version-safe) and cached in
  # :persistent_term; a node bounce with a changed pin re-registers under a
  # fresh md5, so the stale cache entry can only produce a false mismatch,
  # never a false pass.
  @spec fun_compat(node(), timeout()) :: :ok | {:error, term()}
  defp fun_compat(target, timeout) do
    local = :erl_eval.module_info(:md5)

    if :persistent_term.get(cache_key(target), nil) == local do
      :ok
    else
      try do
        register_probe(
          target,
          :erpc.call(target, :erl_eval, :module_info, [:md5], timeout),
          local
        )
      catch
        kind, reason -> {:error, {kind, reason}}
      end
    end
  end

  @spec cache_key(node()) :: tuple()
  defp cache_key(target), do: {__MODULE__, :erl_eval_md5, target}

  @spec register_probe(node(), binary(), binary()) :: :ok | {:error, term()}
  defp register_probe(target, remote, local) do
    with :ok <- compat_error(local, remote, target) do
      :persistent_term.put(cache_key(target), local)
      :ok
    end
  end

  @doc false
  # Exposed for the unit test; not part of the API surface.
  @spec compat_error(binary(), binary(), node()) :: :ok | {:error, term()}
  def compat_error(local, remote, target) do
    if local == remote do
      :ok
    else
      {:error,
       {:erl_eval_mismatch,
        "erl_eval md5 differs between this kernel (#{Base.encode16(local)}) and #{target} " <>
          "(#{Base.encode16(remote)}): the erlang pins have diverged. Update the stale side " <>
          "(rebuild/switch the kernel, or redeploy the fleet) so both run the same nix erlang, " <>
          "or pass the code as a string, which needs no matching bytecode."}}
    end
  end

  @spec remote_multicall([node()], code(), timeout()) :: [term()]
  defp remote_multicall(targets, code, timeout) when is_binary(code) do
    # Unwrap eval's {value, binding} here, on the string path only: a fun
    # returning a 2-tuple must come through untouched.
    targets
    |> :erpc.multicall(Code, :eval_string, [code], timeout)
    |> Enum.map(fn
      {:ok, {value, _binding}} -> {:ok, value}
      other -> other
    end)
  end

  defp remote_multicall(targets, fun, timeout) when is_function(fun, 0) do
    # Per-node compat split: stale nodes get the instructive error, compatible
    # ones run the fun; one bad node cannot poison the batch. The md5 probe
    # fans out in parallel (cached nodes skip it).
    local = :erl_eval.module_info(:md5)

    {cached, unchecked} =
      Enum.split_with(targets, &(:persistent_term.get(cache_key(&1), nil) == local))

    probes = :erpc.multicall(unchecked, :erl_eval, :module_info, [:md5], timeout)

    checks =
      Map.new(
        Enum.zip_with(unchecked, probes, fn t, probe ->
          result =
            case normalize(probe) do
              {:ok, remote} -> register_probe(t, remote, local)
              {:error, reason} -> {:error, reason}
            end

          {t, result}
        end)
      )

    ok_targets = cached ++ for {t, :ok} <- checks, do: t
    results = Map.new(Enum.zip(ok_targets, :erpc.multicall(ok_targets, fun, timeout)))

    Enum.map(targets, fn t ->
      case Map.get(checks, t, :ok) do
        :ok -> Map.fetch!(results, t)
        {:error, reason} -> {:error, reason}
      end
    end)
  end

  @spec least_loaded([node()], timeout()) :: {:ok, node()} | {:error, :no_reachable_nodes}
  defp least_loaded(targets, timeout) do
    targets
    |> :erpc.multicall(:erlang, :statistics, [:run_queue], timeout)
    |> Enum.zip(targets)
    |> Enum.flat_map(fn
      {{:ok, queue}, target} -> [{queue, target}]
      {_other, _target} -> []
    end)
    |> case do
      [] -> {:error, :no_reachable_nodes}
      loads -> {:ok, elem(Enum.min_by(loads, &elem(&1, 0)), 1)}
    end
  end

  @spec start_distribution() :: {:ok, node()} | {:error, term()}
  defp start_distribution do
    case Node.start(local_name(), :longnames) do
      {:ok, _pid} ->
        set_cookie()
        {:ok, node()}

      {:error, {:already_started, _pid}} ->
        set_cookie()
        {:ok, node()}

      {:error, reason} ->
        {:error, reason}
    end
  end

  @spec local_name() :: atom()
  defp local_name do
    case System.get_env(@local_name_env) do
      name when is_binary(name) and name != "" ->
        # Operator-set env; Node.start/2 requires an atom, minted once at boot.
        # astlog-ignore: no-unsafe-to-atom
        String.to_atom(name)

      _ ->
        {:ok, host} = :inet.gethostname()
        # astlog-ignore: no-unsafe-to-atom -- same: one boot-time atom
        String.to_atom("mcp-ex@" <> to_string(host))
    end
  end

  @spec set_cookie() :: :ok
  defp set_cookie do
    case cookie() do
      nil -> :ok
      # astlog-ignore: no-unsafe-to-atom -- deploy-injected secret; the API takes an atom
      value -> Node.set_cookie(String.to_atom(value))
    end

    :ok
  end

  @spec cookie() :: String.t() | nil
  defp cookie do
    cond do
      value = System.get_env(@cookie_env) -> value
      path = System.get_env(@cookie_file_env) -> read_cookie_file(path)
      true -> nil
    end
  end

  @spec read_cookie_file(String.t()) :: String.t() | nil
  defp read_cookie_file(path) do
    case File.read(path) do
      {:ok, contents} -> String.trim(contents)
      {:error, _} -> nil
    end
  end

  @spec normalize(term()) :: exec_result()
  defp normalize({:ok, value}), do: {:ok, value}
  defp normalize({:error, reason}), do: {:error, reason}
  defp normalize({:throw, value}), do: {:error, {:throw, value}}
  defp normalize({:exit, reason}), do: {:error, {:exit, reason}}
end
