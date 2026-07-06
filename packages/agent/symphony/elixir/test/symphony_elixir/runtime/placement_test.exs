defmodule SymphonyElixir.Runtime.PlacementTest do
  use ExUnit.Case, async: false

  alias SymphonyElixir.Config
  alias SymphonyElixir.Runtime.Placement

  # A direct-connect config so the lifecycle resolves a VM address rather
  # than opening a real port-forward Port. `room: %{registry_url: nil}` keeps
  # the room-registry calls inert (no HTTP). No real `ix` runs: every
  # command goes through the injected stub driver below.
  # The example pack's single-repo manifest, so the clone script the
  # lifecycle builds has a real catalog to render without booting Config.
  @repositories_file Path.expand("../../../../workflows/example/repositories.yaml", __DIR__)

  defp config(overrides \\ %{}) do
    base = %Config{
      ix_command: "ix",
      ix_image: "ix/symphony-codex:test",
      ix_room_server_command: "room-server",
      ix_region: nil,
      ix_room_port: 8080,
      ix_room_connect: "direct",
      ix_local_port_base: 18_080,
      ix_keep_vm?: false,
      ix_create_timeout_ms: 120_000,
      ix_env_passthrough: [],
      github_token: nil,
      github_app_bot_username: nil,
      github_app_bot_email: nil,
      repositories_file: @repositories_file,
      room: %{server_url: nil, registry_url: nil, registry_token: nil, advertise_host: nil},
      placement_fallback: :host,
      host_user: "agentuser",
      host_group: nil,
      host_workspaces_dir: nil,
      host_room_server_command: "room-server",
      host_systemd_run_command: "systemd-run",
      host_keep?: false
    }

    struct(base, overrides)
  end

  # A driver that records each `ix` argv it is handed and answers from a
  # fixed VM record, so the acquire/release path is exercised with no VM
  # and no shell-out. `wait_for_room` always succeeds.
  defp recording_driver(test_pid) do
    %{
      ix_cmd: fn _config, args, _timeout ->
        send(test_pid, {:ix_cmd, args})
        :ok
      end,
      ix_vm_by_name: fn _config, vm_name ->
        {:ok, %{"name" => vm_name, "ipv4" => "10.0.0.5"}}
      end,
      wait_for_room: fn _url, _timeout -> :ok end,
      port_forward: fn _config, _vm, _mapping -> {:error, :should_not_port_forward_in_direct_mode} end,
      stop_port_forward: fn _port -> :ok end
    }
  end

  # A driver that records each `systemd-run`/`systemctl` argv and answers
  # the host lifecycle from fixed values: a `getent passwd` line with a
  # home, a fixed port, and a healthy room. No real unit is ever started.
  defp host_driver(test_pid) do
    %{
      ix_cmd: fn _config, args, _timeout ->
        send(test_pid, {:ix_cmd, args})
        :ok
      end,
      ix_vm_by_name: fn _config, vm_name -> {:ok, %{"name" => vm_name, "ipv4" => "10.0.0.5"}} end,
      wait_for_room: fn _url, _timeout -> :ok end,
      port_forward: fn _config, _vm, _mapping -> {:error, :unused} end,
      stop_port_forward: fn _port -> :ok end,
      host_passwd: fn _config, user -> {:ok, "#{user}:x:1000:1000::/home/#{user}:/bin/bash"} end,
      systemd_run: fn _config, args, _timeout ->
        send(test_pid, {:systemd_run, args})
        :ok
      end,
      systemctl_stop: fn unit ->
        send(test_pid, {:systemctl_stop, unit})
        :ok
      end,
      pick_port: fn -> 41_234 end
    }
  end

  setup do
    # Fresh registry table per test; the supervised Placement process is
    # not started here, so the module creates the table lazily on write.
    if :ets.whereis(:symphony_placement) != :undefined do
      :ets.delete(:symphony_placement)
    end

    :ok
  end

  test "acquire provisions a per-run room-server and resolves its base url" do
    opts = [config: config(), driver: recording_driver(self())]

    assert {:ok, "http://10.0.0.5:8080"} = Placement.acquire("run_alpha", :ixvm, opts)
    assert {:ok, "http://10.0.0.5:8080"} = Placement.base_url("run_alpha")

    # The first ix command is the create; it names the run's VM and image.
    assert_received {:ix_cmd, ["new", "ix/symphony-codex:test", "--name", vm_name, "--l7-proxy-port", "8080", "--no-shell"]}
    assert String.starts_with?(vm_name, "sym-run-alpha-")
  end

  test "create_vm is invoked with config.ix_create_timeout_ms, not a hardcoded constant" do
    test_pid = self()
    configured_timeout = 30_000

    # The driver records every ix_cmd call with its timeout argument so we
    # can assert the timeout threaded to the driver matches the config value.
    timeout_recording_driver = %{
      ix_cmd: fn _config, args, timeout ->
        send(test_pid, {:ix_cmd, args, timeout})
        :ok
      end,
      ix_vm_by_name: fn _config, vm_name -> {:ok, %{"name" => vm_name, "ipv4" => "10.0.0.5"}} end,
      wait_for_room: fn _url, _timeout -> :ok end,
      port_forward: fn _config, _vm, _mapping -> {:error, :unused} end,
      stop_port_forward: fn _port -> :ok end
    }

    opts = [config: config(%{ix_create_timeout_ms: configured_timeout}), driver: timeout_recording_driver]

    assert {:ok, _url} = Placement.acquire("run_timeout_check", :ixvm, opts)

    # The first ix_cmd call is the `ix new` (create). Assert its timeout
    # matches the config value, not the old 15-minute module constant.
    assert_received {:ix_cmd, ["new" | _], ^configured_timeout}
  end

  test "acquire is idempotent: a second call returns the same url without re-provisioning" do
    opts = [config: config(), driver: recording_driver(self())]

    assert {:ok, url} = Placement.acquire("run_beta", :ixvm, opts)

    # Drain the create/shell commands from the first acquire.
    drain_ix_cmds()

    assert {:ok, ^url} = Placement.acquire("run_beta", :ixvm, opts)

    # No further ix commands on the second acquire.
    refute_received {:ix_cmd, _args}
  end

  test "release tears the vm down and drops the per-run url" do
    test_pid = self()
    opts = [config: config(), driver: recording_driver(test_pid)]

    assert {:ok, _url} = Placement.acquire("run_gamma", :ixvm, opts)
    drain_ix_cmds()

    assert :ok = Placement.release("run_gamma", opts)
    assert :error = Placement.base_url("run_gamma")

    # Release removes the VM by name.
    assert_received {:ix_cmd, ["rm", "--force", vm_name]}
    assert String.starts_with?(vm_name, "sym-run-gamma-")
  end

  test "release is a no-op for a run that never acquired a placement" do
    assert :ok = Placement.release("run_never", config: config(), driver: recording_driver(self()))
    refute_received {:ix_cmd, _args}
  end

  test "base_url is :error for an unknown run" do
    assert :error = Placement.base_url("run_unknown")
  end

  test "a setup failure surfaces as ixvm_setup_failed and removes the partial vm" do
    failing_driver = %{
      ix_cmd: fn _config, args, _timeout ->
        send(self(), {:ix_cmd, args})

        case args do
          ["new" | _] -> {:error, {:ix_cli_failed, args, 1, "boom"}}
          _ -> :ok
        end
      end,
      ix_vm_by_name: fn _config, vm_name -> {:ok, %{"name" => vm_name, "ipv4" => "10.0.0.5"}} end,
      wait_for_room: fn _url, _timeout -> :ok end,
      port_forward: fn _config, _vm, _mapping -> {:error, :unused} end,
      stop_port_forward: fn _port -> :ok end
    }

    # placement_fallback: :none isolates the raw ixvm failure path; the
    # ixvm -> host fallback is covered by its own describe block.
    assert {:error, {:ixvm_setup_failed, _reason}} =
             Placement.acquire("run_delta", :ixvm, config: config(placement_fallback: :none), driver: failing_driver)

    assert :error = Placement.base_url("run_delta")
  end

  describe "host placement" do
    test "acquire provisions a per-run systemd-run room-server and resolves its loopback url" do
      opts = [config: config(), driver: host_driver(self())]

      assert {:ok, "http://127.0.0.1:41234"} = Placement.acquire("run_host", {:host, "box"}, opts)
      assert {:ok, "http://127.0.0.1:41234"} = Placement.base_url("run_host")

      # The first systemd-run is the workspace clone, in a named "-setup" unit
      # under the polkit-scoped prefix, dropping privileges to the host user.
      assert_received {:systemd_run, setup_args}
      assert "--uid=agentuser" in setup_args
      assert Enum.any?(setup_args, &String.starts_with?(&1, "--unit=symphony-host-"))
      assert Enum.any?(setup_args, &String.ends_with?(&1, "-setup.service"))

      # The second is the long-lived room-server unit (no --wait).
      assert_received {:systemd_run, room_args}
      refute "--wait" in room_args
      assert Enum.any?(room_args, &String.starts_with?(&1, "--unit=symphony-host-"))
      assert "room-server" in room_args or Enum.any?(room_args, &String.ends_with?(&1, "room-server"))
    end

    test "an advertised host binds and resolves a reachable url instead of loopback" do
      base = config()
      cfg = %{base | room: %{base.room | advertise_host: "100.0.0.7"}}
      opts = [config: cfg, driver: host_driver(self())]

      # The registered/resolved url uses the advertised host so the central
      # room.ix.dev can reach the per-run server (not 127.0.0.1).
      assert {:ok, "http://100.0.0.7:41234"} = Placement.acquire("run_adv", {:host, "box"}, opts)
      assert {:ok, "http://100.0.0.7:41234"} = Placement.base_url("run_adv")

      # The room-server unit actually binds that host (--host 100.0.0.7), not
      # only advertises it.
      assert_received {:systemd_run, _setup_args}
      assert_received {:systemd_run, room_args}
      assert "100.0.0.7" in room_args
    end

    test "the minted bot token authors the clone auth and room-server env over the static host token" do
      opts = [config: config(github_token: "human-token"), driver: host_driver(self()), bot_token: "app-token"]

      assert {:ok, _url} = Placement.acquire("run_bot_token", :host, opts)

      # The clone runs in the "-setup" unit; its script stamps the App token
      # as the git auth header, never the static host token.
      assert_received {:systemd_run, setup_args}
      setup_script = List.last(setup_args)
      assert setup_script =~ Base.encode64("x-access-token:app-token")
      refute setup_script =~ Base.encode64("x-access-token:human-token")

      # gh pr create authors as GH_TOKEN, so the long-lived room-server unit
      # must carry the App token in both GitHub vars (ENG-2012).
      assert_received {:systemd_run, room_args}
      assert "--setenv=GITHUB_TOKEN=app-token" in room_args
      assert "--setenv=GH_TOKEN=app-token" in room_args
      refute Enum.any?(room_args, &(&1 =~ "human-token"))
    end

    test "release stops the unit and removes the checkout" do
      opts = [config: config(), driver: host_driver(self())]

      assert {:ok, _url} = Placement.acquire("run_host2", :host, opts)
      drain_systemd_runs()

      assert :ok = Placement.release("run_host2", opts)
      assert :error = Placement.base_url("run_host2")

      assert_received {:systemctl_stop, unit}
      assert String.starts_with?(unit, "symphony-host-")
      assert String.ends_with?(unit, ".service")

      # Cleanup runs as a "-clean" sync unit under the same prefix.
      assert_received {:systemd_run, clean_args}
      assert Enum.any?(clean_args, &String.ends_with?(&1, "-clean.service"))
    end

    test "host_keep? leaves the unit and checkout in place on release" do
      opts = [config: config(host_keep?: true), driver: host_driver(self())]

      assert {:ok, _url} = Placement.acquire("run_keep", :host, opts)
      drain_systemd_runs()

      assert :ok = Placement.release("run_keep", opts)
      refute_received {:systemctl_stop, _unit}
      refute_received {:systemd_run, _args}
    end

    test "host setup fails fast when the host user is not configured" do
      opts = [config: config(host_user: nil), driver: host_driver(self())]

      assert {:error, {:host_setup_failed, :host_user_not_configured}} =
               Placement.acquire("run_nouser", :host, opts)

      assert :error = Placement.base_url("run_nouser")
    end
  end

  describe "ixvm -> host fallback" do
    test "an ixvm setup failure falls back to a host room-server under the same run id" do
      failing_ixvm =
        Map.put(host_driver(self()), :ix_cmd, fn _config, args, _timeout ->
          send(self(), {:ix_cmd, args})

          case args do
            ["new" | _] -> {:error, {:ix_cli_failed, args, 1, "no capacity"}}
            _ -> :ok
          end
        end)

      opts = [config: config(placement_fallback: :host), driver: failing_ixvm]

      # The node declared :ixvm; provisioning fails and the run completes on
      # a host room-server resolved under the same run id, so the engine
      # turn (which looks up by run id) never knows it fell back.
      assert {:ok, "http://127.0.0.1:41234"} = Placement.acquire("run_fb", :ixvm, opts)
      assert {:ok, "http://127.0.0.1:41234"} = Placement.base_url("run_fb")
    end

    test "fallback :local resolves to no per-run placement (the client uses the default url)" do
      failing_ixvm =
        Map.put(host_driver(self()), :ix_cmd, fn _config, args, _timeout ->
          case args do
            ["new" | _] -> {:error, {:ix_cli_failed, args, 1, "boom"}}
            _ -> :ok
          end
        end)

      opts = [config: config(placement_fallback: :local), driver: failing_ixvm]

      assert {:error, {:no_placement_needed, :local}} = Placement.acquire("run_fb_local", :ixvm, opts)
      assert :error = Placement.base_url("run_fb_local")
    end

    test "fallback :none leaves the original ixvm setup failure standing" do
      failing_ixvm =
        Map.put(host_driver(self()), :ix_cmd, fn _config, args, _timeout ->
          case args do
            ["new" | _] -> {:error, {:ix_cli_failed, args, 1, "boom"}}
            _ -> :ok
          end
        end)

      opts = [config: config(placement_fallback: :none), driver: failing_ixvm]

      assert {:error, {:ixvm_setup_failed, _reason}} = Placement.acquire("run_fb_none", :ixvm, opts)
      assert :error = Placement.base_url("run_fb_none")
    end
  end

  describe "ixvm -> remote fallback" do
    # A driver whose ixvm provisioning fails, with the remote seam wired to a
    # fake worker so the fallback runs without a real registry or channel.
    defp remote_driver(test_pid, overrides \\ %{}) do
      worker = %{worker_id: "w1", pid: test_pid, address: "100.0.0.9", labels: [], capacity: 0, registered_at: 0}

      Map.merge(
        %{
          ix_cmd: fn _config, args, _timeout ->
            case args do
              ["new" | _] -> {:error, {:ix_cli_failed, args, 1, "boom"}}
              _ -> :ok
            end
          end,
          ix_vm_by_name: fn _config, vm_name -> {:ok, %{"name" => vm_name}} end,
          wait_for_room: fn _url, _timeout -> :ok end,
          worker_select: fn label ->
            send(test_pid, {:worker_select, label})
            {:ok, worker}
          end,
          worker_get: fn _id -> {:ok, worker} end,
          worker_provision: fn ^worker, run_id, spec, _timeout ->
            send(test_pid, {:worker_provision, run_id, spec})
            {:ok, %{base_url: "http://100.0.0.9:9100", primary_workspace: "/home/hari/symphony-workspaces/#{run_id}/example"}}
          end,
          worker_teardown: fn ^worker, run_id, _timeout ->
            send(test_pid, {:worker_teardown, run_id})
            :ok
          end
        },
        overrides
      )
    end

    test "an ixvm failure falls back to a remote worker's room-server" do
      opts = [config: config(placement_fallback: :remote), driver: remote_driver(self())]

      assert {:ok, "http://100.0.0.9:9100"} = Placement.acquire("run_rem", :ixvm, opts)
      assert {:ok, %{location: :remote, base_url: "http://100.0.0.9:9100"}} = Placement.resolved("run_rem")
      assert_received {:worker_select, nil}
      assert_received {:worker_provision, "run_rem", %{env: _, bot_token: _}}
    end

    test "the dispatched spec carries the run's repository catalog so the worker clones the real repos" do
      config = config(placement_fallback: :remote)
      opts = [config: config, driver: remote_driver(self())]

      assert {:ok, _url} = Placement.acquire("run_rem_repos", :ixvm, opts)
      assert_received {:worker_provision, "run_rem_repos", %{repositories: repositories}}
      assert repositories == SymphonyElixir.RepositoryCatalog.all(config)
      assert repositories != []
    end

    test "the dispatched spec carries the bot commit identity so the worker clone authors as the App" do
      opts = [
        config:
          config(
            placement_fallback: :remote,
            github_app_bot_username: "ix-playbook-agent[bot]",
            github_app_bot_email: "ix-playbook-agent[bot]@users.noreply.github.com"
          ),
        driver: remote_driver(self())
      ]

      assert {:ok, _url} = Placement.acquire("run_rem_bot", :ixvm, opts)

      assert_received {:worker_provision, "run_rem_bot",
                       %{
                         bot_username: "ix-playbook-agent[bot]",
                         bot_email: "ix-playbook-agent[bot]@users.noreply.github.com"
                       }}
    end

    test "select uses the configured worker label" do
      opts = [config: config(placement_fallback: :remote, worker_select_label: "hari"), driver: remote_driver(self())]

      assert {:ok, _url} = Placement.acquire("run_rem_lbl", :ixvm, opts)
      assert_received {:worker_select, "hari"}
    end

    test "a remote placement resolves the worker-side primary checkout as its cwd" do
      opts = [config: config(placement_fallback: :remote), driver: remote_driver(self())]
      assert {:ok, _url} = Placement.acquire("run_rem_cwd", :ixvm, opts)

      assert {:ok, "/home/hari/symphony-workspaces/run_rem_cwd/example"} =
               Placement.workspace_cwd("run_rem_cwd", opts)
    end

    test "release dispatches teardown to the worker" do
      opts = [config: config(placement_fallback: :remote), driver: remote_driver(self())]
      assert {:ok, _url} = Placement.acquire("run_rem_rel", :ixvm, opts)

      assert :ok = Placement.release("run_rem_rel", opts)
      assert_received {:worker_teardown, "run_rem_rel"}
      assert :error = Placement.base_url("run_rem_rel")
    end

    test "no connected worker surfaces the original ixvm failure" do
      driver = remote_driver(self(), %{worker_select: fn _label -> {:error, :no_worker} end})
      opts = [config: config(placement_fallback: :remote), driver: driver]

      assert {:error, {:ixvm_setup_failed, _reason}} = Placement.acquire("run_rem_none", :ixvm, opts)
      assert :error = Placement.base_url("run_rem_none")
    end
  end

  describe "workspace_cwd/2" do
    test "a host placement resolves the primary-repo checkout under the host run root" do
      opts = [config: config(), driver: host_driver(self())]
      assert {:ok, _url} = Placement.acquire("run_cwd_host", :host, opts)

      assert {:ok, "/home/agentuser/symphony-workspaces/run_cwd_host/example"} =
               Placement.workspace_cwd("run_cwd_host", opts)
    end

    test "an ixvm placement resolves the VM-internal primary-repo checkout" do
      opts = [config: config(), driver: recording_driver(self())]
      assert {:ok, _url} = Placement.acquire("run_cwd_ix", :ixvm, opts)

      assert {:ok, "/workspace/symphony/run_cwd_ix/example"} =
               Placement.workspace_cwd("run_cwd_ix", opts)
    end

    test "an ixvm node that fell back to host resolves the host checkout" do
      failing_ixvm =
        Map.put(host_driver(self()), :ix_cmd, fn _config, args, _timeout ->
          case args do
            ["new" | _] -> {:error, {:ix_cli_failed, args, 1, "no capacity"}}
            _ -> :ok
          end
        end)

      opts = [config: config(placement_fallback: :host), driver: failing_ixvm]
      assert {:ok, _url} = Placement.acquire("run_cwd_fb", :ixvm, opts)

      # The declared location was :ixvm, but the cwd follows the effective
      # host placement so the turn runs where the clone actually landed.
      assert {:ok, "/home/agentuser/symphony-workspaces/run_cwd_fb/example"} =
               Placement.workspace_cwd("run_cwd_fb", opts)
    end

    test "a run with no acquired placement has no cwd" do
      assert :error = Placement.workspace_cwd("run_cwd_none")
    end
  end

  defp drain_ix_cmds do
    receive do
      {:ix_cmd, _args} -> drain_ix_cmds()
    after
      0 -> :ok
    end
  end

  defp drain_systemd_runs do
    receive do
      {:systemd_run, _args} -> drain_systemd_runs()
      {:ix_cmd, _args} -> drain_systemd_runs()
    after
      0 -> :ok
    end
  end

  describe "reconcile/2" do
    test "reaps an orphaned host unit and re-attaches a live one" do
      units = %{
        "symphony-host-live.service" => {"run_live", 1111},
        "symphony-host-dead.service" => {"run_dead", 2222}
      }

      graphs = [graph("run_live", :running), graph("run_dead", :succeeded)]
      opts = [config: config(), driver: reconcile_driver(self(), units)]

      assert :ok = Placement.reconcile(graphs, opts)

      # The terminal run's server is stopped and its checkout cleaned; the
      # live run's server is left running.
      assert_received {:systemctl_stop, "symphony-host-dead.service"}
      refute_received {:systemctl_stop, "symphony-host-live.service"}
      assert_received {:systemd_run, clean_args}
      assert "--unit=symphony-host-dead-clean.service" in clean_args

      # The live run is re-attached so a resumed acquire resolves to the
      # existing server instead of provisioning a duplicate.
      assert {:ok, "http://127.0.0.1:1111"} = Placement.base_url("run_live")
      # The reaped run holds no placement.
      assert :error = Placement.base_url("run_dead")
    end

    test "reaps a unit whose run is absent from the store" do
      units = %{"symphony-host-ghost.service" => {"run_ghost", 3333}}
      opts = [config: config(), driver: reconcile_driver(self(), units)]

      assert :ok = Placement.reconcile([], opts)
      assert_received {:systemctl_stop, "symphony-host-ghost.service"}
    end

    test "is a no-op when the host user is unconfigured" do
      units = %{"symphony-host-x.service" => {"run_x", 4444}}
      opts = [config: config(%{host_user: nil}), driver: reconcile_driver(self(), units)]

      assert :ok = Placement.reconcile([graph("run_x", :running)], opts)
      refute_received {:systemctl_stop, _unit}
    end
  end

  defp graph(run_id, status) do
    %SymphonyElixir.IR.RunGraph{run_id: run_id, source_hash: "hash", status: status, nodes: %{}}
  end

  # A driver answering the reconcile path from a fixed unit table. Each
  # entry maps a unit name to its `{run_id, port}`; `systemctl_show_exec_start`
  # renders the same `ExecStart` shape systemd reports (the run id is the
  # `--state-dir` basename), and `systemctl_stop`/`systemd_run` record so a
  # test can assert exactly which units were reaped.
  defp reconcile_driver(test_pid, units) do
    %{
      host_passwd: fn _config, user -> {:ok, "#{user}:x:1000:1000::/home/#{user}:/bin/bash"} end,
      systemctl_list_host_units: fn -> Map.keys(units) end,
      systemctl_show_exec_start: fn unit ->
        {run_id, port} = Map.fetch!(units, unit)

        {:ok,
         "{ path=/n/room-server ; argv[]=/n/room-server --host 127.0.0.1 --port #{port} " <>
           "--state-dir /home/agentuser/.local/state/symphony-room/#{run_id} ; ignore_errors=no }"}
      end,
      systemctl_stop: fn unit ->
        send(test_pid, {:systemctl_stop, unit})
        :ok
      end,
      systemd_run: fn _config, args, _timeout ->
        send(test_pid, {:systemd_run, args})
        :ok
      end
    }
  end
end
