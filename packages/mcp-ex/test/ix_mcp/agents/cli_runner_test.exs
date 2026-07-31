defmodule IxMcp.Agents.CliRunnerTest do
  # async: false: exercises the application's shared harness and ledger.
  use ExUnit.Case, async: false

  import IxMcpTest.Eventually

  alias IxMcp.Agents
  alias IxMcp.Agents.Control

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

  describe "interrupt (#4486)" do
    # A stub that answers the SDK control frame and nothing else: it reads the
    # injected brief, then waits for the interrupt before finishing. So a final
    # result proves the frame arrived on the child's stdin, rather than proving
    # only that nothing crashed.
    defp interrupt_stub!(tmp) do
      path = Path.join(tmp, "stub-interrupt")

      File.write!(path, """
      #!/bin/sh
      read _brief
      printf '%s\\n' '{"type":"system","subtype":"init","session_id":"sid-int"}'
      while read line; do
        case "$line" in
          *control_request*)
            printf '%s\\n' '{"type":"result","result":"stopped","is_error":false}'
            exit 0
            ;;
        esac
      done
      """)

      File.chmod!(path, 0o755)
      path
    end

    @tag :tmp_dir
    test "reaches the child's stdin through the runner that owns the port", %{tmp_dir: tmp} do
      bin = interrupt_stub!(tmp)
      {:ok, id} = Agents.spawn("wait for it", backend: :claude, bin: bin, name: "rt-interrupt")

      # The runner registers as it starts; interrupting before that is the
      # :not_running case, so wait for the handle rather than racing it.
      eventually(fn -> if match?({:ok, _entry}, Control.lookup(id)), do: true end)

      assert :ok = Agents.interrupt(id)
      assert {:ok, "stopped"} = Agents.await(id, 10_000)
      assert :interrupt in (id |> Agents.events() |> Enum.map(& &1.kind))

      # A finished child idles rather than terminating (the lead can wake it),
      # so it holds one of the harness's four concurrency slots until deleted.
      Agents.delete(id)
    end

    @tag :tmp_dir
    test "a codex child says it has no channel instead of pretending", %{tmp_dir: tmp} do
      bin = stub!(tmp, "codex-oneshot.jsonl")
      {:ok, id} = Agents.spawn("say ok", backend: :codex, bin: bin, name: "rt-int-codex")

      # Either the runner is still up (no stdin channel) or the one-shot child
      # already finished (nothing to interrupt). Both are honest answers; a bare
      # :ok would not be.
      assert Agents.interrupt(id) in [{:error, :no_stdin_channel}, {:error, :not_running}]

      Agents.delete(id)
    end

    test "an unknown child is not running, not silently accepted" do
      assert {:error, :not_running} = Agents.interrupt("never-spawned-at-all")
    end
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
