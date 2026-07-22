defmodule IxMcp.Agents.CliRunnerTest do
  # async: false: exercises the application's shared harness and ledger.
  use ExUnit.Case, async: false

  alias IxMcp.Agents

  @fixtures Path.expand("../../fixtures", __DIR__)

  defp stub!(tmp, fixture) do
    path = Path.join(tmp, "stub-#{fixture}")
    File.write!(path, "#!/bin/sh\nexec cat #{Path.join(@fixtures, fixture)}\n")
    File.chmod!(path, 0o755)
    path
  end

  @tag :tmp_dir
  test "claude child: recorded stream to final, events, graph", %{tmp_dir: tmp} do
    bin = stub!(tmp, "claude-oneshot.ndjson")
    {:ok, id} = Agents.spawn("say ok", backend: :claude, bin: bin, name: "rt-claude")

    assert {:ok, "ok"} = Agents.await(id, 10_000)
    assert {:ok, final} = Agents.report() |> Map.fetch("rt-claude")
    assert final == {:ok, "ok"}

    kinds = id |> Agents.events() |> Enum.map(& &1.kind)
    assert :init in kinds
    assert :result in kinds
    assert :text in kinds

    assert %{nodes: nodes, edges: edges} = Agents.graph()
    assert Enum.any?(nodes, &(&1["id"] == "rt-claude" and &1["state"] == "done"))
    assert ["lead", "rt-claude"] in edges
  end

  @tag :tmp_dir
  test "codex child: a failed turn surfaces as an error to the lead", %{tmp_dir: tmp} do
    bin = stub!(tmp, "codex-oneshot.jsonl")
    {:ok, id} = Agents.spawn("say ok", backend: :codex, bin: bin, name: "rt-codex")

    assert {:error, message} = Agents.await(id, 10_000)
    assert message =~ "out of credits"

    assert %{nodes: nodes, edges: _} = Agents.graph()
    assert Enum.any?(nodes, &(&1["id"] == "rt-codex" and &1["state"] == "blocked"))
  end

  # The #3989 shape (Cmd's #3979, fixed in #3985): Erlang's child setup
  # reports a failed chdir by exiting with the raw errno, so a missing
  # cwd came back as {:exit_status, 2} -- no output, ENOENT dressed up as
  # the CLI's own exit. The runner now raises before the spawn; the crash
  # reaches the lead as a named error instead of a bare status.
  describe "missing cwd (#3989)" do
    # A stub that waits for the injected user line, removes its own cwd,
    # and exits as told: the deterministic after-the-spawn (TOCTOU) shape.
    defp rmdir_stub!(tmp, exit_code) do
      path = Path.join(tmp, "stub-rmdir-#{exit_code}")
      File.write!(path, "#!/bin/sh\nread _line\nrmdir \"$PWD\"\nexit #{exit_code}\n")
      File.chmod!(path, 0o755)
      path
    end

    defp workdir!(tmp) do
      dir = Path.join(tmp, "workdir-#{System.unique_integer([:positive])}")
      File.mkdir_p!(dir)
      dir
    end

    # Spawn, await, and delete: the live-subagent cap (max_concurrent)
    # counts idle finished children, so each probe must free its slot.
    defp run_child(brief, opts) do
      {:ok, id} = Agents.spawn(brief, opts)
      result = Agents.await(id, 10_000)
      Agents.delete(id)
      result
    end

    @tag :tmp_dir
    @tag capture_log: true
    test "spawn works while the cwd exists and raises once it is deleted", %{tmp_dir: tmp} do
      bin = stub!(tmp, "claude-oneshot.ndjson")
      dir = workdir!(tmp)

      assert {:ok, "ok"} =
               run_child("say ok", backend: :claude, bin: bin, cwd: dir, name: "rt-cwd-ok")

      File.rm_rf!(dir)

      assert {:error, message} =
               run_child("say ok", backend: :claude, bin: bin, cwd: dir, name: "rt-cwd-gone")

      assert message =~ "runner_crash"
      assert message =~ "cwd #{dir} does not exist"
    end

    @tag :tmp_dir
    @tag capture_log: true
    test "a cwd that is a file, not a directory, raises (was errno 20)", %{tmp_dir: tmp} do
      bin = stub!(tmp, "claude-oneshot.ndjson")
      file = Path.join(tmp, "plain-file")
      File.write!(file, "")

      assert {:error, message} =
               run_child("say ok", backend: :claude, bin: bin, cwd: file, name: "rt-cwd-file")

      assert message =~ "cwd #{file} is not a directory"
    end

    @tag :tmp_dir
    @tag capture_log: true
    test "a cwd deleted mid-run with a nonzero exit raises instead of an ambiguous status",
         %{tmp_dir: tmp} do
      bin = rmdir_stub!(tmp, 3)
      dir = workdir!(tmp)

      assert {:error, message} =
               run_child("go", backend: :claude, bin: bin, cwd: dir, name: "rt-cwd-race")

      assert message =~ "cwd #{dir} no longer exists"
      assert message =~ "raw chdir errno"
    end

    @tag :tmp_dir
    test "exit 0 is never second-guessed, even with the cwd gone", %{tmp_dir: tmp} do
      bin = rmdir_stub!(tmp, 0)
      dir = workdir!(tmp)

      assert {:error, message} =
               run_child("go", backend: :claude, bin: bin, cwd: dir, name: "rt-cwd-clean")

      assert message =~ "exit_status, 0"
      refute message =~ "errno"
    end

    @tag :tmp_dir
    @tag capture_log: true
    test "a deleted launch dir raises with the launch-dir hint", %{tmp_dir: tmp} do
      bin = stub!(tmp, "claude-oneshot.ndjson")

      doomed = Path.join(System.tmp_dir!(), "ix-cli-doomed-#{System.unique_integer([:positive])}")
      File.mkdir_p!(doomed)
      before = File.cwd!()
      File.cd!(doomed)
      IxMcp.Cmd.capture_launch_cwd()
      File.cd!(before)
      # File.cwd!() resolves symlinks (/tmp -> /private/tmp on macOS), so
      # the captured path is the canonical spelling, not `doomed` verbatim.
      captured = IxMcp.Cmd.launch_cwd()

      try do
        File.rm_rf!(doomed)

        assert {:error, message} =
                 run_child("say ok", backend: :claude, bin: bin, name: "rt-launch-gone")

        assert message =~ "cwd #{captured} does not exist (session launch dir deleted?)"
      after
        # Recapture the real launch dir so no later test inherits the stub.
        IxMcp.Cmd.capture_launch_cwd()
        File.rm_rf!(doomed)
      end
    end
  end
end
