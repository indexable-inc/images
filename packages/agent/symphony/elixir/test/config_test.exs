defmodule SymphonyElixir.ConfigTest do
  use ExUnit.Case, async: false

  alias SymphonyElixir.Config

  test "captures default codex runtime knobs" do
    config = Config.get()

    assert config.ix_command == "ix"
    assert config.ix_image == "ix/symphony-codex:latest"
    assert config.ix_room_server_command == "room-server"
    assert config.ix_room_port == 8080
    assert config.ix_room_connect == "direct"
    assert config.ix_local_port_base == 18_080
    refute config.ix_keep_vm?
    assert config.ix_env_passthrough == ["OPENAI_API_KEY", "CODEX_API_KEY"]
    assert config.host_user == nil
    assert config.host_group == nil
    assert config.host_workspaces_dir == nil
    assert config.host_room_server_command == "room-server"
    assert config.host_systemd_run_command == "systemd-run"
    refute config.host_keep?
  end

  test "reads the room advertise host and registry url from the environment" do
    original = Config.get()

    on_exit(fn ->
      System.delete_env("SYMPHONY_ROOM_ADVERTISE_HOST")
      System.delete_env("SYMPHONY_ROOM_REGISTRY_URL")
      restart_config!(original)
    end)

    System.put_env("SYMPHONY_ROOM_ADVERTISE_HOST", "100.0.0.7")
    System.put_env("SYMPHONY_ROOM_REGISTRY_URL", "https://room.ix.dev")
    restart_config!(original)

    config = Config.get()
    assert config.room.advertise_host == "100.0.0.7"
    assert config.room.registry_url == "https://room.ix.dev"
  end

  test "creates mutable runtime dirs without mutating workflow pack assets" do
    original = Config.get()
    root = tmp_dir("config_pack_state")
    pack_dir = write_pack!(Path.join(root, "pack"))
    workspaces_dir = Path.join(root, "state/workspaces")
    runs_dir = Path.join(root, "state/runs")

    on_exit(fn -> restart_config!(original) end)
    restart_config!(root: root, pack_dir: pack_dir, workspaces_dir: workspaces_dir, runs_dir: runs_dir)

    assert File.dir?(workspaces_dir)
    assert File.dir?(runs_dir)
    assert File.dir?(Path.join(pack_dir, "workflows"))
    assert File.dir?(Path.join(pack_dir, "skills"))
  end

  test "fails clearly when workflow pack assets are missing" do
    original = Config.get()
    root = tmp_dir("config_missing_pack_asset")
    pack_dir = Path.join(root, "pack")
    File.mkdir_p!(Path.join(pack_dir, "skills"))
    File.write!(Path.join(pack_dir, "repositories.yaml"), "repositories: []\n")

    on_exit(fn -> restart_config!(original) end)
    stop_config()

    previous_flag = Process.flag(:trap_exit, true)

    assert {:error, {%RuntimeError{message: message}, _stack}} =
             Config.start_link(root: root, pack_dir: pack_dir)

    receive do
      {:EXIT, _pid, {%RuntimeError{}, _stack}} -> :ok
    after
      0 -> :ok
    end

    Process.flag(:trap_exit, previous_flag)

    assert message =~ "SYMPHONY_WORKFLOWS_DIR must point at an existing directory"
    refute File.exists?(Path.join(pack_dir, "workflows"))
  end

  defp write_pack!(pack_dir) do
    File.mkdir_p!(Path.join(pack_dir, "workflows"))
    File.mkdir_p!(Path.join(pack_dir, "skills"))
    File.write!(Path.join(pack_dir, "repositories.yaml"), "repositories: []\n")
    pack_dir
  end

  defp restart_config!(%Config{} = snapshot) do
    opts =
      snapshot
      |> Map.from_struct()
      |> Map.to_list()

    restart_config!(opts)
  end

  defp restart_config!(opts) do
    stop_config()
    assert {:ok, pid} = Config.start_link(opts)
    Process.unlink(pid)
  end

  defp stop_config do
    case Process.whereis(Config) do
      nil ->
        :ok

      pid ->
        ref = Process.monitor(pid)
        GenServer.stop(pid, :normal)

        receive do
          {:DOWN, ^ref, :process, ^pid, _reason} -> :ok
        after
          1_000 -> flunk("timed out stopping SymphonyElixir.Config")
        end
    end
  end

  defp tmp_dir(name) do
    dir = Path.join(System.tmp_dir!(), "symphony_#{name}_#{System.unique_integer([:positive])}")
    File.mkdir_p!(dir)
    on_exit(fn -> File.rm_rf!(dir) end)
    dir
  end
end
