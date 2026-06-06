defmodule SymphonyElixir.Runtime.HostRuntime do
  @moduledoc """
  Spawns and tears down a per-run room-server on *this* host.

  The mechanics of a host placement - clone a run-scoped workspace, start a
  privilege-dropped `systemd-run` unit bound to `room_host:port`, health-poll
  it, and clean it up on teardown - live here as one implementation, used two
  ways:

    * in-process by `Runtime.Placement` for a local `:host` placement, where
      the room-server binds loopback (`room_host` = `"127.0.0.1"`); and
    * inside a `Runtime.WorkerClient` that provisions on its own host on behalf
      of a remote control plane, where the room-server binds the worker's
      reachable address (`room_host` = the worker's advertised host) so the
      control plane can reach it over the engine wire.

  It owns no registry. The caller records the returned `handle`: the control
  plane in its ETS table and on `room.ix.dev`, the worker in its channel reply.
  The low-level host ops go through the same injectable driver seam as
  `Runtime.Placement` (`host_passwd`, `systemd_run`, `systemctl_stop`,
  `pick_port`, `wait_for_room`, ...), so tests exercise the lifecycle without
  `systemd-run`. The unit names share the `symphony-host-` prefix the polkit
  grant in `modules/services/symphony.nix` scopes to.
  """

  alias SymphonyElixir.{Command, Config}
  alias SymphonyElixir.Codex.Provision

  require Logger

  @default_setup_timeout_ms 10 * 60 * 1000
  @default_start_timeout_ms 30 * 1000
  @default_stop_timeout_ms 60 * 1000
  @default_health_timeout_ms 60 * 1000

  # The active per-run room units, by bare unit name. Matches only the
  # "symphony-host-<hash>.service" room units, not the "-setup"/"-clean" sync
  # units (those are oneshot and gone).
  @host_room_unit ~r/^symphony-host-[0-9a-f]+\.service$/

  @typedoc """
  A provisioned host room-server: the `base_url` the engine wire targets, the
  `systemd-run` `unit`, the dropped-to `user`/`home`, and the `run_root` to
  remove on teardown.
  """
  @type handle :: %{
          base_url: String.t(),
          unit: String.t(),
          user: String.t(),
          home: String.t(),
          run_root: String.t(),
          primary_workspace: String.t()
        }

  @typedoc """
  The subset of a provisioned handle `teardown/2` consumes: the `unit` to stop
  and the `user`/`home`/`run_root` to reap. An open map so both a full
  `handle()` (WorkerClient) and the smaller record `Runtime.Placement`
  reconstructs (no `primary_workspace`) satisfy the contract.
  """
  @type teardown_handle :: %{
          :unit => String.t(),
          :user => String.t(),
          :home => String.t(),
          :run_root => String.t(),
          optional(any()) => any()
        }

  @doc """
  Provision the run's room-server on this host bound to `room_host`
  (`opts[:room_host]`, default `"127.0.0.1"`). Returns `{:ok, handle}` or a
  bare `{:error, reason}`; the caller wraps the reason in its own contract
  (`Placement` as `{:host_setup_failed, reason}`).
  """
  @spec provision(String.t(), keyword()) :: {:ok, handle()} | {:error, term()}
  def provision(run_id, opts \\ []) when is_binary(run_id) and is_list(opts) do
    config = config(opts)
    driver = driver(opts)
    bind_host = room_host(opts)

    with {:ok, user} <- host_user(config),
         {:ok, home} <- host_home(config, driver, user) do
      run_root = Provision.host_run_root(config, home, run_id)
      state_dir = Provision.host_room_state_dir(home, run_id)
      base = Provision.host_unit_base(run_id, "room")
      unit = base <> ".service"
      port = driver.pick_port.()
      url = "http://#{bind_host}:#{port}"

      Logger.info("HostRuntime: creating unit=#{unit} url=#{url} user=#{user} run=#{run_id}")

      # The resolved host identity (config, driver seam, dropped-to user, and
      # that user's home) is shared by both setup and start; pass it as one
      # named context so neither helper crosses the credo arity ceiling.
      host = %{config: config, driver: driver, user: user, home: home}

      with :ok <- setup_workspace(host, run_root, state_dir, base, run_id, opts),
           :ok <- start_room_server(host, bind_host, state_dir, unit, port, url, opts) do
        Logger.info("HostRuntime: ready unit=#{unit} url=#{url} run=#{run_id}")

        primary_workspace =
          Provision.host_primary_workspace(config, run_root, run_id, Keyword.get(opts, :repositories))

        {:ok,
         %{
           base_url: url,
           unit: unit,
           user: user,
           home: home,
           run_root: run_root,
           primary_workspace: primary_workspace
         }}
      else
        {:error, reason} ->
          # Stop a half-started unit so a failed provision does not leave a
          # room-server bound to the port.
          driver.systemctl_stop.(unit)
          {:error, reason}
      end
    end
  end

  @doc """
  Tear down a previously provisioned `handle`: stop the unit and remove its
  checkout. A no-op-safe `keep?` (default `config.host_keep?`) leaves the unit
  up for inspection. Idempotent.
  """
  @spec teardown(teardown_handle(), keyword()) :: :ok
  def teardown(%{} = handle, opts \\ []) when is_list(opts) do
    config = config(opts)
    driver = driver(opts)

    if Keyword.get(opts, :keep?, config.host_keep?) do
      Logger.info("HostRuntime: keeping unit=#{handle.unit} for inspection")
    else
      driver.systemctl_stop.(handle.unit)

      cleanup_workspace(config, driver, %{
        host_unit: handle.unit,
        host_user: handle.user,
        host_home: handle.home,
        host_run_root: handle.run_root
      })
    end

    :ok
  end

  @doc """
  Remove a run's checkout via a `systemd-run` cleanup unit under the same
  `symphony-host-` prefix the polkit grant authorizes. Used by teardown and by
  reconcile's reaping path.
  """
  @spec cleanup_workspace(Config.t(), map(), %{
          host_unit: String.t(),
          host_user: String.t(),
          host_home: String.t(),
          host_run_root: String.t()
        }) :: :ok
  def cleanup_workspace(%Config{} = config, driver, placement) do
    base = String.replace_suffix(placement.host_unit, ".service", "")
    unit = base <> "-clean.service"
    script = Provision.host_cleanup_script(placement.host_run_root)

    args =
      Provision.host_run_sync_args(config, placement.host_user, placement.host_home, unit, [], [
        bash_executable(),
        "-lc",
        script
      ])

    case driver.systemd_run.(config, args, @default_stop_timeout_ms) do
      :ok -> :ok
      {:error, reason} -> Logger.warning("HostRuntime: cleanup failed unit=#{unit}: #{inspect(reason)}")
    end

    :ok
  end

  @doc "The configured host user, or `{:error, :host_user_not_configured}`."
  @spec host_user(Config.t()) :: {:ok, String.t()} | {:error, term()}
  def host_user(%Config{host_user: user}) when is_binary(user) and user != "", do: {:ok, user}
  def host_user(%Config{}), do: {:error, :host_user_not_configured}

  @doc "Resolve the target user's home from `getent passwd` via the driver."
  @spec host_home(Config.t(), map(), String.t()) :: {:ok, Path.t()} | {:error, term()}
  def host_home(%Config{} = config, driver, user) do
    case driver.host_passwd.(config, user) do
      {:ok, output} -> Provision.parse_passwd_home(output, user)
      {:error, reason} -> {:error, {:host_user_lookup_failed, user, reason}}
    end
  end

  @doc """
  The default host portion of the placement driver: the real `systemd-run`,
  `getent`, `systemctl`, room-health, and free-port implementations. Merged
  into `Runtime.Placement`'s driver and used as this module's default.
  """
  @spec default_driver() :: map()
  def default_driver do
    %{
      host_passwd: &real_host_passwd/2,
      systemd_run: &real_systemd_run/3,
      systemctl_stop: &real_systemctl_stop/1,
      systemctl_list_host_units: &real_systemctl_list_host_units/0,
      systemctl_show_exec_start: &real_systemctl_show_exec_start/1,
      wait_for_room: &Provision.wait_for_room/2,
      pick_port: &real_pick_port/0
    }
  end

  # --- internals ------------------------------------------------------

  defp setup_workspace(%{config: config, driver: driver, user: user, home: home}, run_root, state_dir, base, run_id, opts) do
    token = Keyword.get(opts, :bot_token) || config.github_token
    script = Provision.host_workspace_script(config, run_root, state_dir, run_id, token, Keyword.get(opts, :repositories))
    unit = base <> "-setup.service"
    args = Provision.host_run_sync_args(config, user, home, unit, [], [bash_executable(), "-lc", script])
    driver.systemd_run.(config, args, @default_setup_timeout_ms)
  end

  defp start_room_server(%{config: config, driver: driver, user: user, home: home}, bind_host, state_dir, unit, port, url, opts) do
    # A remote worker receives the run's env already resolved from the control
    # plane (it holds no secrets itself), so an explicit `opts[:env]` overrides
    # the local `Provision.runtime_env` resolution.
    env = Keyword.get(opts, :env) || Provision.runtime_env(config, opts)
    cmd = Provision.host_room_server_command(config, bind_host, port, state_dir)
    args = Provision.host_run_unit_args(config, user, home, unit, env, cmd)

    case driver.systemd_run.(config, args, @default_start_timeout_ms) do
      :ok ->
        case driver.wait_for_room.(url, @default_health_timeout_ms) do
          :ok -> :ok
          {:error, reason} -> {:error, {:room_start_failed, reason}}
        end

      {:error, reason} ->
        {:error, {:room_start_failed, reason}}
    end
  end

  defp room_host(opts), do: Keyword.get(opts, :room_host, "127.0.0.1")

  defp config(opts), do: Keyword.get_lazy(opts, :config, &Config.get/0)

  defp driver(opts), do: Map.merge(default_driver(), Keyword.get(opts, :driver, %{}))

  # --- host driver ----------------------------------------------------

  defp real_host_passwd(%Config{}, user) do
    case Command.run(getent_executable(), ["passwd", user], 5_000) do
      {:ok, output} -> {:ok, output}
      {:error, reason} -> {:error, reason}
    end
  end

  defp real_systemd_run(%Config{} = config, args, timeout_ms) do
    case Command.run(systemd_run_executable(config), args, timeout_ms) do
      {:ok, _output} -> :ok
      {:error, {:exit, status, output}} -> {:error, {:systemd_run_failed, Provision.sanitize_setenv_args(args), status, String.trim(output)}}
      {:error, {:timeout, ms, output}} -> {:error, {:systemd_run_timeout, Provision.sanitize_setenv_args(args), ms, String.trim(output)}}
      {:error, {:start_failed, reason}} -> {:error, {:systemd_run_error, Provision.sanitize_setenv_args(args), reason}}
    end
  end

  defp real_systemctl_stop(nil), do: :ok

  defp real_systemctl_stop(unit) when is_binary(unit) do
    case Command.run(systemctl_executable(), ["stop", unit], @default_stop_timeout_ms) do
      {:ok, _output} -> Logger.info("HostRuntime: stopped unit=#{unit}")
      {:error, reason} -> Logger.warning("HostRuntime: failed to stop unit=#{unit}: #{inspect(reason)}")
    end

    :ok
  end

  defp real_systemctl_list_host_units do
    args = ["list-units", "--type=service", "--all", "--plain", "--no-legend", "symphony-host-*.service"]

    case Command.run(systemctl_executable(), args, @default_stop_timeout_ms) do
      {:ok, output} ->
        output
        |> String.split("\n", trim: true)
        |> Enum.map(fn line -> line |> String.trim() |> String.split(~r/\s+/, trim: true) |> List.first() end)
        |> Enum.filter(&(is_binary(&1) and Regex.match?(@host_room_unit, &1)))

      {:error, reason} ->
        Logger.warning("HostRuntime: failed to list host units: #{inspect(reason)}")
        []
    end
  end

  defp real_systemctl_show_exec_start(unit) when is_binary(unit) do
    case Command.run(systemctl_executable(), ["show", unit, "--property=ExecStart", "--value"], @default_stop_timeout_ms) do
      {:ok, output} -> {:ok, String.trim(output)}
      {:error, reason} -> {:error, reason}
    end
  end

  # A free port chosen by the OS for the host room-server. The bind/close
  # window is a small TOCTOU race against the unit binding the same port;
  # acceptable because the port space is large and one run provisions one
  # server.
  defp real_pick_port do
    {:ok, socket} = :gen_tcp.listen(0, [:binary, ip: {127, 0, 0, 1}, reuseaddr: true])
    {:ok, port} = :inet.port(socket)
    :gen_tcp.close(socket)
    port
  end

  defp systemd_run_executable(%Config{host_systemd_run_command: command}) do
    System.find_executable(command) || command
  end

  defp systemctl_executable do
    System.find_executable("systemctl") || "systemctl"
  end

  defp getent_executable do
    System.find_executable("getent") || "getent"
  end

  # Resolve bash to an absolute path: a transient unit's default PATH does not
  # include the Nix store, so a bare "bash" would fail to exec on NixOS.
  defp bash_executable do
    System.find_executable("bash") || "bash"
  end
end
