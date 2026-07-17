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

  Code is sent to remote nodes as **source strings** evaluated with
  `Code.eval_string/1`, not as closures. Closures would serialize as
  module + hash and only resolve if both sides run byte-identical modules;
  strings sidestep that entirely and keep ad-hoc cells simple.

      Fleet.exec(:"beamd@host-a.tailnet.ts.net", "System.schedulers_online()")
      Fleet.exec_any("node() |> to_string()")            # a random node
      Fleet.multicall(Fleet.nodes(), "node()", timeout: 10_000)
  """

  @nodes_env "IX_BEAM_NODES"
  @cookie_env "IX_BEAM_COOKIE"
  @cookie_file_env "IX_BEAM_COOKIE_FILE"
  @local_name_env "IX_BEAM_LOCAL_NAME"

  @default_timeout 15_000

  @type exec_result :: {:ok, term()} | {:error, term()}

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
  Evaluate `code` on `target` and return `{:ok, value}` or `{:error, reason}`.
  A dead or unreachable node fails only this call. `:timeout` (ms) defaults to
  #{@default_timeout}.
  """
  @spec exec(node(), String.t(), keyword()) :: exec_result()
  def exec(target, code, opts \\ []) when is_atom(target) and is_binary(code) do
    timeout = Keyword.get(opts, :timeout, @default_timeout)

    with {:ok, _} <- ensure_dist() do
      try do
        {value, _binding} = :erpc.call(target, Code, :eval_string, [code], timeout)
        {:ok, value}
      catch
        kind, reason -> {:error, {kind, reason}}
      end
    end
  end

  @doc """
  Evaluate `code` on one random configured node. `{:error, :no_nodes}` when
  `IX_BEAM_NODES` is empty.
  """
  @spec exec_any(String.t(), keyword()) :: exec_result()
  def exec_any(code, opts \\ []) when is_binary(code) do
    case nodes() do
      [] -> {:error, :no_nodes}
      ns -> exec(Enum.random(ns), code, opts)
    end
  end

  @doc """
  Evaluate `code` on the least-loaded reachable node (by scheduler run-queue
  length). `{:error, :no_nodes}` when none are configured.
  """
  @spec exec_least_loaded(String.t(), keyword()) :: exec_result()
  def exec_least_loaded(code, opts \\ []) when is_binary(code) do
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
  @spec multicall([node()], String.t(), keyword()) :: [{node(), exec_result()}]
  def multicall(targets, code, opts \\ []) when is_list(targets) and is_binary(code) do
    timeout = Keyword.get(opts, :timeout, @default_timeout)

    case ensure_dist() do
      {:ok, _} ->
        targets
        |> :erpc.multicall(Code, :eval_string, [code], timeout)
        |> Enum.zip(targets)
        |> Enum.map(fn {result, target} -> {target, normalize(result)} end)

      {:error, reason} ->
        Enum.map(targets, &{&1, {:error, reason}})
    end
  end

  # -- internals ------------------------------------------------------------

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
  defp normalize({:ok, {value, _binding}}), do: {:ok, value}
  defp normalize({:error, reason}), do: {:error, reason}
  defp normalize({:throw, value}), do: {:error, {:throw, value}}
  defp normalize({:exit, reason}), do: {:error, {:exit, reason}}
end
