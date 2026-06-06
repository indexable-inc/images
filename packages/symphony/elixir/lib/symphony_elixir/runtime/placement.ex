defmodule SymphonyElixir.Runtime.Placement do
  @moduledoc """
  Owns the per-run room-server lifecycle for the IR engine path.

  A run whose agent nodes declare `location: :ixvm` gets its own
  room-server process living in a short-lived iXVM (its own cgroup/PID per
  the PRD), provisioned once before the run's first agent turn and torn
  down when the run ends. `Engine.Client.resolve_base_url/2` reads the
  resolved per-run URL back from here, so a turn routes to the run's own
  server rather than the shared `SYMPHONY_ROOM_SERVER_URL`.

  This is the IR successor to the lifecycle the legacy `Codex.IxVm` ran
  per node turn. The load-bearing shell construction (the `ix new`/`shell`
  argv, the clone and room-start scripts, the port-forward mapping, the
  secret redaction) is reused from `Codex.Provision` so this path and the
  legacy path build the same commands and redact the same way. The
  difference is the unit of work: one room-server serves the whole run and
  speaks the engine wire (`/api/agent/turns`), rather than one VM per node
  driving the old `/api/workflow/turns` poll loop.

  ## Registry

  Resolved placements live in a public named ETS table keyed by `run_id`,
  so the `Engine.Client` (which runs inside a monitored attempt task, off
  the runtime process) can read the URL without a GenServer round-trip.
  The table is owned by this process so it is reclaimed if the supervisor
  restarts the placement registry.

  ## Driver seam (no real VMs in tests)

  Every `ix` invocation, the room-health poll, and the port-forward tunnel
  go through an injectable driver (`opts[:driver]`), defaulting to the
  real implementation. Tests pass a stub driver so the lifecycle logic is
  exercised without spawning a microVM or shelling out to `ix`.

  ## `host` placement and the `ixvm -> host` fallback

  A run whose agent nodes declare `location: {:host, _}` gets its per-run
  room-server as a privilege-dropped `systemd-run` unit on this host (its
  own cgroup/PID, no VM), the IR successor to the per-node lifecycle in
  `Codex.Host`. The unit names share the `symphony-host-` prefix the polkit
  grant in `modules/services/symphony` scopes to, so the non-root Symphony
  service is authorized to manage them.

  When `:ixvm` provisioning fails before the first turn, `acquire/3` retries
  on the fallback placement read from `Config.placement_fallback` (default
  `:host`). The fallback URL is registered under the same `run_id`, so a
  turn carrying an `:ixvm` envelope resolves to the host room-server without
  the node knowing it fell back. `:local` stays the dev convenience (drop to
  the in-process server); `:none` disables the fallback and the run fails
  against the missing placement.

  ## Known limitations

  Setup is synchronous and can take minutes (`ix new` plus a clone, or the
  host clone unit). It runs on the runtime's behalf before the first turn,
  so a run blocks on provisioning. A setup failure with no usable fallback
  surfaces as `{:error, {:ixvm_setup_failed, reason}}` /
  `{:error, {:host_setup_failed, reason}}`.
  """

  use GenServer

  alias SymphonyElixir.{Command, Config, RepositoryCatalog}
  alias SymphonyElixir.Codex.{Provision, RoomRegistry}
  alias SymphonyElixir.Runtime.{HostRuntime, RuntimeRegistry, WorkerDispatch}

  require Logger

  @table :symphony_placement
  @default_setup_timeout_ms 10 * 60 * 1000
  @default_health_timeout_ms 60 * 1000
  # A remote provision covers a worker-side clone + room-server start, so it
  # shares the generous setup budget rather than a short RPC default.
  @default_remote_timeout_ms 10 * 60 * 1000

  @typedoc """
  A resolved per-run placement: the base URL the engine wire targets, the
  location tag the room-server actually runs on (the effective placement
  after any `ixvm -> host`/`ixvm -> remote` fallback, not the node's declared
  location), and the teardown handles for that placement.

  iXVM placements carry `vm_name` and the port-forward `Port`; host placements
  carry the `systemd-run` `unit`, the `user`/`home` the unit runs as, and the
  `run_root` to remove on teardown; remote placements carry the `worker_id` to
  dispatch teardown to and the worker-side `remote_cwd` the engine turn runs in.
  Each path leaves the other paths' handles `nil`.
  """
  @type placement :: %{
          base_url: String.t(),
          location: :ixvm | :host | :remote,
          vm_name: String.t() | nil,
          port_forward: port() | nil,
          backend_id: String.t() | nil,
          host_unit: String.t() | nil,
          host_user: String.t() | nil,
          host_home: String.t() | nil,
          host_run_root: String.t() | nil,
          worker_id: String.t() | nil,
          remote_cwd: String.t() | nil
        }

  @spec start_link(keyword()) :: GenServer.on_start()
  def start_link(opts \\ []) do
    GenServer.start_link(__MODULE__, opts, name: __MODULE__)
  end

  @impl true
  def init(_opts) do
    :ets.new(@table, [:named_table, :public, :set, read_concurrency: true])
    {:ok, %{}}
  end

  @doc """
  Resolve a per-run base URL for `location`, provisioning the run's
  room-server if `location` needs one. Idempotent per run: a second call
  for an already-resolved `run_id` returns the same URL without
  re-provisioning, so the runtime can acquire lazily before any agent
  turn without tracking whether it already did.

  Returns `{:ok, base_url}` or `{:error, reason}`. `opts` carries
  `:config` (defaults to the boot snapshot) and an injectable `:driver`
  for tests.
  """
  @spec acquire(String.t(), SymphonyElixir.Engine.Envelope.location(), keyword()) ::
          {:ok, String.t()} | {:error, term()}
  def acquire(run_id, location, opts \\ []) when is_binary(run_id) and is_list(opts) do
    case lookup(run_id) do
      {:ok, %{base_url: base_url}} -> {:ok, base_url}
      :error -> provision(run_id, location, opts)
    end
  end

  @doc """
  The per-run base URL the engine wire should target, or `:error` if no
  placement was acquired for this run. Read by `Engine.Client` when an
  envelope's location is `:ixvm`.
  """
  @spec base_url(String.t()) :: {:ok, String.t()} | :error
  def base_url(run_id) when is_binary(run_id) do
    case lookup(run_id) do
      {:ok, %{base_url: base_url}} -> {:ok, base_url}
      :error -> :error
    end
  end

  @doc "The resolved placement for a run (effective location after fallback), or :error."
  @spec resolved(String.t()) :: {:ok, %{location: :ixvm | :host | :remote, base_url: String.t()}} | :error
  def resolved(run_id) when is_binary(run_id) do
    case lookup(run_id) do
      {:ok, %{location: location, base_url: base_url}} -> {:ok, %{location: location, base_url: base_url}}
      :error -> :error
    end
  end

  @doc """
  The working directory an agent turn runs in: the run's primary-repo
  checkout for the resolved placement. The clone landed there during
  acquire, so the engine turn must run from the same path the room-server
  can see. `:host` checks out under the target user's home and `:ixvm` at
  the VM-internal workspace root, which differ by placement; reading the
  path back from the stored record keeps the cwd consistent with where the
  clone actually went after any `ixvm -> host` fallback.

  Returns `:error` when no placement was acquired (a `:local`/`:room` run),
  so the caller can decide what an agent turn with no resolved checkout
  means rather than this module inventing a path.
  """
  @spec workspace_cwd(String.t(), keyword()) :: {:ok, String.t()} | :error
  def workspace_cwd(run_id, opts \\ []) when is_binary(run_id) do
    case lookup(run_id) do
      {:ok, %{location: :host, host_run_root: run_root}} when is_binary(run_root) ->
        {:ok, Provision.host_primary_workspace(config(opts), run_root, run_id)}

      {:ok, %{location: :ixvm}} ->
        {:ok, Provision.ix_primary_workspace(config(opts), run_id)}

      {:ok, %{location: :remote, remote_cwd: cwd}} when is_binary(cwd) ->
        {:ok, cwd}

      _ ->
        :error
    end
  end

  @doc """
  Tear down a run's placement: stop the port-forward, unregister the room
  backend, remove the VM, and drop the registry entry. Idempotent and a
  no-op for a run that never acquired one (a `:local`/`:room` run), so the
  runtime can call it unconditionally at run end.
  """
  @spec release(String.t(), keyword()) :: :ok
  def release(run_id, opts \\ []) when is_binary(run_id) and is_list(opts) do
    case lookup(run_id) do
      {:ok, placement} ->
        teardown(run_id, placement, config(opts), driver(opts))
        :ets.delete(table(), run_id)
        :ok

      :error ->
        :ok
    end
  end

  @doc """
  Reap host room-server units left orphaned by a BEAM restart, and
  re-attach the ones whose run is still live.

  The placement registry is in-memory ETS, so a restart loses every
  resolved placement: units started before the restart can no longer be
  found by `release/2`, and a resumed run that re-`acquire`s would collide
  on the deterministic `symphony-host-<hash>` unit name. `reconcile/2`
  rebuilds that state from the host. For each live `symphony-host-*.service`
  room unit it recovers the `run_id` from the `--state-dir` in the unit's
  `ExecStart`, then:

    * if the run is non-terminal (`:pending`/`:running` - the same set
      `Supervisor.resume_pending/1` resumes), re-inserts the placement into
      the registry so the resumed run re-attaches to the existing server
      instead of provisioning a duplicate;
    * otherwise stops the unit, removes its checkout, and unregisters its
      room backend so a terminal run's server and its room.ix.dev entry do
      not linger.

  `graphs` is the full run set from `IR.Store.load_all/0`; the caller loads
  it once and shares it with `resume_pending/1`. Host placements only -
  iXVM reaping is not handled here. Idempotent and a no-op when no host
  units exist or the host user is unconfigured.
  """
  @spec reconcile([SymphonyElixir.IR.RunGraph.t()], keyword()) :: :ok
  def reconcile(graphs, opts \\ []) when is_list(graphs) and is_list(opts) do
    config = config(opts)
    driver = driver(opts)

    live =
      for %{run_id: run_id, status: status} <- graphs,
          status in [:pending, :running],
          into: MapSet.new(),
          do: run_id

    with {:ok, user} <- HostRuntime.host_user(config),
         {:ok, home} <- HostRuntime.host_home(config, driver, user) do
      host = %{config: config, driver: driver, user: user, home: home}

      driver.systemctl_list_host_units.()
      |> Enum.each(&reconcile_unit(&1, host, live))
    else
      {:error, reason} ->
        Logger.warning("Placement: reconcile skipped, host identity unresolved: #{inspect(reason)}")
    end

    :ok
  end

  defp reconcile_unit(unit, %{driver: driver} = host, live) when is_binary(unit) do
    case unit_run(driver, unit) do
      {:ok, run_id, port, state_dir} ->
        if MapSet.member?(live, run_id) do
          reattach_unit(host, unit, run_id, port)
        else
          reap_unit(host, unit, run_id, state_dir)
        end

      :error ->
        Logger.warning("Placement: reconcile leaving #{unit}; could not recover its run from ExecStart")
    end
  end

  # A live non-terminal run owns this unit: register the existing server so
  # the resumed run's `acquire` short-circuits to it rather than colliding
  # on the deterministic unit name.
  defp reattach_unit(%{config: config, user: user, home: home}, unit, run_id, port) do
    placement = %{
      base_url: "http://127.0.0.1:#{port}",
      location: :host,
      vm_name: nil,
      port_forward: nil,
      backend_id: Provision.backend_id(run_id, "room"),
      host_unit: unit,
      host_user: user,
      host_home: home,
      host_run_root: Provision.host_run_root(config, home, run_id),
      worker_id: nil,
      remote_cwd: nil
    }

    :ets.insert(table(), {run_id, placement})
    Logger.info("Placement: reconcile re-attached host unit=#{unit} run=#{run_id}")
  end

  # No live run owns this unit: stop the server, drop its checkout, and
  # unregister its room.ix.dev backend.
  defp reap_unit(%{config: config, driver: driver, user: user, home: home}, unit, run_id, _state_dir) do
    Logger.info("Placement: reconcile reaping orphaned host unit=#{unit} run=#{run_id}")
    driver.systemctl_stop.(unit)
    RoomRegistry.unregister(config, Provision.backend_id(run_id, "room"))

    HostRuntime.cleanup_workspace(config, driver, %{
      host_unit: unit,
      host_user: user,
      host_home: home,
      host_run_root: Provision.host_run_root(config, home, run_id)
    })
  end

  # The room unit's `ExecStart` carries the bind port and the per-run state
  # dir whose basename is the run id (the unit name is a non-reversible
  # hash, so the run is recovered from the state dir, not the name).
  defp unit_run(driver, unit) do
    case driver.systemctl_show_exec_start.(unit) do
      {:ok, exec_start} -> parse_room_exec_start(exec_start)
      {:error, _reason} -> :error
    end
  end

  defp parse_room_exec_start(exec_start) when is_binary(exec_start) do
    args = String.split(exec_start, ~r/\s+/, trim: true)

    with {:ok, state_dir} <- exec_flag(args, "--state-dir"),
         {:ok, port_str} <- exec_flag(args, "--port"),
         {port, ""} <- Integer.parse(port_str),
         run_id when run_id != "" <- Path.basename(state_dir) do
      {:ok, run_id, port, state_dir}
    else
      _ -> :error
    end
  end

  defp exec_flag(args, flag) do
    case Enum.drop_while(args, &(&1 != flag)) do
      [^flag, value | _] -> {:ok, value}
      _ -> :error
    end
  end

  # --- provisioning ---------------------------------------------------

  # Only :ixvm and host placements run a provisioned per-run server today.
  # :local and {:room, _} resolve to a fixed URL in the client and never
  # acquire a placement, so a call here for one of them is a no-op success:
  # there is nothing to provision and nothing to release.
  defp provision(_run_id, :local, _opts), do: {:error, {:no_placement_needed, :local}}
  defp provision(_run_id, {:room, _}, _opts), do: {:error, {:no_placement_needed, :room}}

  defp provision(run_id, :ixvm, opts) do
    case provision_ixvm(run_id, opts) do
      {:ok, base_url} ->
        {:ok, base_url}

      {:error, {:ixvm_setup_failed, _reason}} = err ->
        fallback(run_id, config(opts).placement_fallback, err, opts)
    end
  end

  # An explicit {:host, _} node placement provisions a host room-server
  # directly. The host carries no payload here: the per-run room-server is
  # named by `run_id`, not by the location's host string (which named a
  # box in the legacy topology, not a per-run server).
  defp provision(run_id, {:host, _}, opts), do: provision_host(run_id, opts)
  defp provision(run_id, :host, opts), do: provision_host(run_id, opts)

  defp provision(_run_id, location, _opts), do: {:error, {:unresolvable_location, location}}

  defp provision_ixvm(run_id, opts) do
    config = config(opts)
    driver = driver(opts)
    vm_name = Provision.vm_name(run_id, "room")

    Logger.info("Placement: creating ixvm vm=#{vm_name} image=#{config.ix_image} run=#{run_id}")

    with {:ok, vm} <- create_vm(config, driver, vm_name, Provision.runtime_env(config, opts)),
         :ok <- setup_workspace(config, driver, vm_name, run_id, opts),
         :ok <- start_room_server(config, driver, vm_name, run_id, opts),
         {:ok, base_url, port_forward} <- room_url(config, driver, vm) do
      backend_id = register_backend(config, run_id, base_url, vm_name, "ixvm")

      placement = %{
        base_url: base_url,
        location: :ixvm,
        vm_name: vm_name,
        port_forward: port_forward,
        backend_id: backend_id,
        host_unit: nil,
        host_user: nil,
        host_home: nil,
        host_run_root: nil,
        worker_id: nil,
        remote_cwd: nil
      }

      :ets.insert(table(), {run_id, placement})
      Logger.info("Placement: ixvm ready vm=#{vm_name} url=#{base_url} run=#{run_id}")
      {:ok, base_url}
    else
      {:error, reason} ->
        # Best-effort cleanup of a half-created VM so a failed acquire does
        # not leak a microVM before the fallback takes over.
        driver.ix_cmd.(config, Provision.rm_vm_args(vm_name), @default_setup_timeout_ms)
        {:error, {:ixvm_setup_failed, reason}}
    end
  end

  # The `ixvm -> fallback` retry, target read from Config (never a .sym
  # literal). :host reprovisions the per-run room-server on this host;
  # :local drops to the in-process default URL (no placement to acquire),
  # so it is a no-op success the client resolves through the default URL;
  # :none leaves the original ixvm failure standing.
  defp fallback(run_id, :host, _ixvm_error, opts) do
    Logger.warning("Placement: ixvm setup failed for run=#{run_id}; falling back to host")
    provision_host(run_id, opts)
  end

  defp fallback(run_id, :remote, ixvm_error, opts) do
    Logger.warning("Placement: ixvm setup failed for run=#{run_id}; falling back to remote worker")

    case provision_remote(run_id, opts) do
      {:ok, base_url} -> {:ok, base_url}
      # No worker is connected: surface the original ixvm failure rather than a
      # confusing "no worker" so the operator sees the real cause.
      {:error, {:remote_setup_failed, :no_worker}} -> ixvm_error
      {:error, _reason} = err -> err
    end
  end

  defp fallback(_run_id, :local, _ixvm_error, _opts), do: {:error, {:no_placement_needed, :local}}
  defp fallback(_run_id, :none, ixvm_error, _opts), do: ixvm_error

  # Provision the run's room-server on a registered remote worker: pick a
  # worker, dispatch the per-run env + clone token to it, and record the
  # worker-bound base_url + worker-side cwd in the registry. HostRuntime runs
  # inside the worker; here we only select, dispatch, and bookkeep.
  defp provision_remote(run_id, opts) do
    config = config(opts)
    driver = driver(opts)

    with {:ok, worker} <- driver.worker_select.(config.worker_select_label),
         spec = %{
           env: Provision.runtime_env(config, opts),
           bot_token: Keyword.get(opts, :bot_token) || config.github_token,
           # The control plane owns the bot identity: it mints the App token
           # above, so it must also ship the matching commit user.name/
           # user.email. A worker holds no bot config of its own, so without
           # these the worker clone keeps its host's personal git identity and
           # the babysit skill's identity guard refuses to push.
           bot_username: config.github_app_bot_username,
           bot_email: config.github_app_bot_email,
           repositories: RepositoryCatalog.all(config)
         },
         {:ok, remote} <- driver.worker_provision.(worker, run_id, spec, @default_remote_timeout_ms) do
      backend_id = register_backend(config, run_id, remote.base_url, nil, "remote")

      placement = %{
        base_url: remote.base_url,
        location: :remote,
        vm_name: nil,
        port_forward: nil,
        backend_id: backend_id,
        host_unit: nil,
        host_user: nil,
        host_home: nil,
        host_run_root: nil,
        worker_id: worker.worker_id,
        remote_cwd: Map.get(remote, :primary_workspace)
      }

      :ets.insert(table(), {run_id, placement})
      Logger.info("Placement: remote ready worker=#{worker.worker_id} url=#{remote.base_url} run=#{run_id}")
      {:ok, remote.base_url}
    else
      {:error, :no_worker} -> {:error, {:remote_setup_failed, :no_worker}}
      {:error, reason} -> {:error, {:remote_setup_failed, reason}}
    end
  end

  defp create_vm(%Config{} = config, driver, vm_name, env) do
    case driver.ix_cmd.(config, Provision.create_vm_args(config, vm_name, env), config.ix_create_timeout_ms) do
      :ok ->
        case driver.ix_vm_by_name.(config, vm_name) do
          {:ok, vm} -> {:ok, Map.put(vm, "id", vm_name)}
          {:error, reason} -> {:error, reason}
        end

      {:error, reason} ->
        {:error, reason}
    end
  end

  defp setup_workspace(%Config{} = config, driver, vm_name, run_id, opts) do
    driver.ix_cmd.(config, Provision.shell_args(vm_name, Provision.ix_workspace_script(config, run_id, opts)), @default_setup_timeout_ms)
  end

  defp start_room_server(%Config{} = config, driver, vm_name, run_id, opts) do
    driver.ix_cmd.(config, Provision.shell_args(vm_name, Provision.ix_room_start_script(config, run_id, opts)), @default_setup_timeout_ms)
  end

  defp room_url(%Config{ix_room_connect: "port_forward"} = config, driver, %{"name" => vm_name}) do
    {mapping, url} = Provision.port_forward_mapping(config, vm_name)

    case driver.port_forward.(config, vm_name, mapping) do
      {:ok, port} ->
        case driver.wait_for_room.(url, @default_health_timeout_ms) do
          :ok ->
            {:ok, url, port}

          {:error, reason} ->
            driver.stop_port_forward.(port)
            {:error, {:room_port_forward_failed, reason}}
        end

      {:error, reason} ->
        {:error, {:room_port_forward_failed, reason}}
    end
  end

  defp room_url(%Config{} = config, driver, %{"name" => vm_name} = vm) do
    with {:ok, address} <- vm_address(config, vm),
         url = direct_room_url(address, config.ix_room_port),
         :ok <- driver.wait_for_room.(url, @default_health_timeout_ms) do
      {:ok, url, nil}
    else
      {:error, reason} -> {:error, {:room_direct_connect_failed, vm_name, reason}}
    end
  end

  defp vm_address(%Config{} = config, vm) do
    case {config.ix_room_connect, Map.get(vm, "ipv4"), Map.get(vm, "ipv6")} do
      {"direct_ipv4", ipv4, _ipv6} when is_binary(ipv4) and ipv4 != "" -> {:ok, {:ipv4, ipv4}}
      {_mode, _ipv4, ipv6} when is_binary(ipv6) and ipv6 != "" -> {:ok, {:ipv6, ipv6}}
      {_mode, ipv4, _ipv6} when is_binary(ipv4) and ipv4 != "" -> {:ok, {:ipv4, ipv4}}
      _ -> {:error, {:vm_has_no_address, Map.get(vm, "name")}}
    end
  end

  defp direct_room_url({:ipv4, address}, port), do: "http://#{address}:#{port}"
  defp direct_room_url({:ipv6, address}, port), do: "http://[#{address}]:#{port}"

  # --- host provisioning ----------------------------------------------

  # Provision the run's room-server on this host via HostRuntime (clone,
  # privilege-dropped systemd-run, health-poll), then record the placement in
  # the registry and on room.ix.dev. HostRuntime owns the host mechanics;
  # Placement owns the registry. room_host stays loopback for a local host
  # placement.
  defp provision_host(run_id, opts) do
    config = config(opts)
    host_opts = opts |> Keyword.put(:driver, driver(opts)) |> maybe_put_room_host(config)

    case HostRuntime.provision(run_id, host_opts) do
      {:ok, host} ->
        backend_id = register_backend(config, run_id, host.base_url, nil, "host")

        placement = %{
          base_url: host.base_url,
          location: :host,
          vm_name: nil,
          port_forward: nil,
          backend_id: backend_id,
          host_unit: host.unit,
          host_user: host.user,
          host_home: host.home,
          host_run_root: host.run_root,
          worker_id: nil,
          remote_cwd: nil
        }

        :ets.insert(table(), {run_id, placement})
        Logger.info("Placement: host ready unit=#{host.unit} url=#{host.base_url} run=#{run_id}")
        {:ok, host.base_url}

      {:error, reason} ->
        {:error, {:host_setup_failed, reason}}
    end
  end

  # Bind the per-run room-server to the configured advertised host instead of
  # loopback so the central room.ix.dev can reach it to proxy the run's
  # transcript. HostRuntime uses room_host for both the bind and the registered
  # base_url, so a reachable host here is what gets registered. An explicit
  # room_host already in opts (tests, or a worker's own address) wins.
  defp maybe_put_room_host(opts, %Config{room: %{advertise_host: host}}) when is_binary(host) and host != "" do
    Keyword.put_new(opts, :room_host, host)
  end

  defp maybe_put_room_host(opts, _config), do: opts

  defp register_backend(%Config{} = config, run_id, base_url, vm_name, runtime) do
    backend_id = Provision.backend_id(run_id, "room")

    RoomRegistry.register(config, %{
      id: backend_id,
      name: Provision.backend_name(%{}, run_id, "room"),
      url: base_url,
      source: "symphony",
      runtime: runtime,
      run_id: run_id,
      node_id: "room",
      vm_name: vm_name,
      status: "active"
    })

    backend_id
  end

  # --- teardown -------------------------------------------------------

  defp teardown(_run_id, %{location: :ixvm} = placement, %Config{} = config, driver) do
    driver.stop_port_forward.(placement.port_forward)

    if placement.backend_id, do: RoomRegistry.unregister(config, placement.backend_id)

    if config.ix_keep_vm? do
      Logger.info("Placement: keeping ixvm vm=#{placement.vm_name} for inspection")
    else
      case driver.ix_cmd.(config, Provision.rm_vm_args(placement.vm_name), @default_setup_timeout_ms) do
        :ok -> Logger.info("Placement: removed ixvm vm=#{placement.vm_name}")
        {:error, reason} -> Logger.warning("Placement: failed to remove vm=#{placement.vm_name}: #{inspect(reason)}")
      end
    end

    :ok
  end

  defp teardown(_run_id, %{location: :host} = placement, %Config{} = config, driver) do
    if placement.backend_id, do: RoomRegistry.unregister(config, placement.backend_id)

    HostRuntime.teardown(
      %{
        base_url: placement.base_url,
        unit: placement.host_unit,
        user: placement.host_user,
        home: placement.host_home,
        run_root: placement.host_run_root
      },
      config: config,
      driver: driver
    )

    :ok
  end

  # Remote teardown is best-effort: unregister the room backend, then ask the
  # worker (if still connected) to stop the run's room-server. A disconnected
  # worker reaps its own orphaned units, so a missing worker is not an error.
  defp teardown(run_id, %{location: :remote} = placement, %Config{} = config, driver) do
    if placement.backend_id, do: RoomRegistry.unregister(config, placement.backend_id)

    case placement.worker_id && driver.worker_get.(placement.worker_id) do
      {:ok, worker} ->
        driver.worker_teardown.(worker, run_id, @default_remote_timeout_ms)

      _ ->
        Logger.info("Placement: remote teardown skipped, worker=#{inspect(placement.worker_id)} not connected run=#{run_id}")
    end

    :ok
  end

  defp teardown(_run_id, _placement, _config, _driver), do: :ok

  # --- registry -------------------------------------------------------

  defp lookup(run_id) do
    case :ets.whereis(@table) do
      :undefined ->
        :error

      _tid ->
        case :ets.lookup(@table, run_id) do
          [{^run_id, placement}] -> {:ok, placement}
          [] -> :error
        end
    end
  end

  # The registry table is created in `init/1`. Tests that exercise the
  # lifecycle without the supervised process create it on first write.
  defp table do
    case :ets.whereis(@table) do
      :undefined -> :ets.new(@table, [:named_table, :public, :set, read_concurrency: true])
      _tid -> @table
    end
  end

  # --- driver ---------------------------------------------------------

  defp config(opts), do: Keyword.get_lazy(opts, :config, &Config.get/0)

  # The real driver: every `ix` call, the health poll, and the
  # port-forward tunnel. Tests override `opts[:driver]` with stubs so no
  # real VM is created. The driver is a plain map of named functions so a
  # test can replace exactly the calls it cares about.
  defp driver(opts), do: Map.merge(default_driver(), Keyword.get(opts, :driver, %{}))

  # The iXVM half of the driver lives here; the host half (systemd-run,
  # getent, systemctl, room-health, free-port) is owned by HostRuntime and
  # merged in, so both the local :host path and a remote worker share one
  # implementation.
  defp default_driver do
    Map.merge(
      %{
        ix_cmd: &real_ix_cmd/3,
        ix_vm_by_name: &real_ix_vm_by_name/2,
        port_forward: &real_port_forward/3,
        stop_port_forward: &real_stop_port_forward/1,
        # The remote half: pick a worker, look one up, and dispatch
        # provision/teardown to it. Tests override these to avoid a real
        # registry or channel.
        worker_select: &RuntimeRegistry.select/1,
        worker_get: &RuntimeRegistry.get/1,
        worker_provision: &WorkerDispatch.provision/4,
        worker_teardown: &WorkerDispatch.teardown/3
      },
      HostRuntime.default_driver()
    )
  end

  defp real_ix_cmd(%Config{} = config, args, timeout_ms) do
    case Command.run(ix_executable(config), args, timeout_ms) do
      {:ok, _output} -> :ok
      {:error, {:exit, status, output}} -> {:error, {:ix_cli_failed, Provision.sanitize_ix_args(args), status, String.trim(output)}}
      {:error, {:timeout, ms, output}} -> {:error, {:ix_cli_timeout, Provision.sanitize_ix_args(args), ms, String.trim(output)}}
      {:error, {:start_failed, reason}} -> {:error, {:ix_cli_error, Provision.sanitize_ix_args(args), reason}}
    end
  end

  defp real_ix_vm_by_name(%Config{} = config, vm_name) do
    with {:ok, output} <- real_ix_cmd_output(config, Provision.list_vms_args(), 30_000),
         {:ok, vms} <- decode_ix_json(output) do
      case Enum.find(vms, &(Map.get(&1, "name") == vm_name)) do
        %{} = vm -> {:ok, vm}
        nil -> {:error, {:ix_vm_not_found, vm_name}}
      end
    end
  end

  defp real_ix_cmd_output(%Config{} = config, args, timeout_ms) do
    case Command.run(ix_executable(config), args, timeout_ms) do
      {:ok, output} -> {:ok, output}
      {:error, {:exit, status, output}} -> {:error, {:ix_cli_failed, Provision.sanitize_ix_args(args), status, String.trim(output)}}
      {:error, {:timeout, ms, output}} -> {:error, {:ix_cli_timeout, Provision.sanitize_ix_args(args), ms, String.trim(output)}}
      {:error, {:start_failed, reason}} -> {:error, {:ix_cli_error, Provision.sanitize_ix_args(args), reason}}
    end
  end

  defp decode_ix_json(output) do
    output
    |> String.trim()
    |> Jason.decode()
    |> case do
      {:ok, list} when is_list(list) -> {:ok, list}
      {:ok, other} -> {:error, {:invalid_ix_cli_payload, other}}
      {:error, reason} -> {:error, {:invalid_ix_cli_json, String.trim(output), reason}}
    end
  end

  defp real_port_forward(%Config{} = config, vm_name, mapping) do
    executable = ix_executable(config)

    port =
      Port.open({:spawn_executable, executable}, [
        :binary,
        :exit_status,
        args: Provision.port_forward_args(vm_name, mapping)
      ])

    {:ok, port}
  rescue
    error -> {:error, {:port_forward_start_failed, Exception.message(error)}}
  end

  defp real_stop_port_forward(nil), do: :ok

  defp real_stop_port_forward(port) when is_port(port) do
    case Port.info(port, :os_pid) do
      {:os_pid, os_pid} -> System.cmd("kill", ["-TERM", Integer.to_string(os_pid)], stderr_to_stdout: true)
      nil -> :ok
    end

    if Port.info(port) != nil, do: Port.close(port)
    :ok
  rescue
    _ -> :ok
  end

  defp ix_executable(%Config{ix_command: command}) do
    System.find_executable(command) || command
  end
end
