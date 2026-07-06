defmodule SymphonyElixir.Runtime.ExecRunnerTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.IR.Node
  alias SymphonyElixir.Runtime.ExecRunner

  setup do
    pack = Path.join(System.tmp_dir!(), "exec_runner_#{System.unique_integer([:positive])}")
    File.mkdir_p!(Path.join(pack, "scripts"))
    on_exit(fn -> File.rm_rf(pack) end)
    # The port's cd resolves symlinks (macOS /tmp is a symlink to
    # /private/tmp), so hand tests the physical path a script's pwd reports.
    {physical, 0} = System.cmd("pwd", [], cd: pack)
    {:ok, pack: String.trim(physical)}
  end

  defp write_script!(pack, rel, body, mode \\ 0o755) do
    path = Path.join(pack, rel)
    File.mkdir_p!(Path.dirname(path))
    File.write!(path, body)
    File.chmod!(path, mode)
    rel
  end

  defp exec_node(rel, opts \\ []) do
    inputs = %{"script" => {:literal, rel}}

    inputs =
      case Keyword.get(opts, :timeout) do
        nil -> inputs
        seconds -> Map.put(inputs, "timeout", {:literal, seconds})
      end

    Node.new(id: "exec-0", ast_origin: "exec-0", kind: :exec, inputs: inputs, state: :pending)
  end

  test "a zero-exit script succeeds and captures output", %{pack: pack} do
    rel = write_script!(pack, "scripts/ok.sh", "#!/bin/sh\necho hello world\n")

    assert {:ok, %{kind: :exec, exit_code: 0, output: output}, nil} =
             ExecRunner.run(exec_node(rel), %{run_id: "r", attempt: 1, pack_dir: pack})

    assert output =~ "hello world"
  end

  test "a non-zero exit fails with the status and output tail", %{pack: pack} do
    rel = write_script!(pack, "scripts/boom.sh", "#!/bin/sh\necho dying\nexit 3\n")

    assert {:error, {:exec_failed, 3, output}, nil} =
             ExecRunner.run(exec_node(rel), %{run_id: "r", attempt: 1, pack_dir: pack})

    assert output =~ "dying"
  end

  test "the script path is resolved against the pack dir, not an absolute deploy path", %{pack: pack} do
    rel = write_script!(pack, "scripts/cwd.sh", "#!/bin/sh\npwd\n")

    assert {:ok, %{output: output}, nil} =
             ExecRunner.run(exec_node(rel), %{run_id: "r", attempt: 1, pack_dir: pack})

    # The script runs with cwd = pack dir.
    assert String.trim(output) == pack
  end

  test "a missing script file fails loudly", %{pack: pack} do
    assert {:error, {:exec_not_found, "scripts/ghost.sh"}, nil} =
             ExecRunner.run(exec_node("scripts/ghost.sh"), %{run_id: "r", attempt: 1, pack_dir: pack})
  end

  test "a non-executable file fails loudly", %{pack: pack} do
    rel = write_script!(pack, "scripts/plain.sh", "#!/bin/sh\ntrue\n", 0o644)

    assert {:error, {:exec_not_executable, ^rel}, nil} =
             ExecRunner.run(exec_node(rel), %{run_id: "r", attempt: 1, pack_dir: pack})
  end

  test "a node missing its script input fails rather than running an empty command", %{pack: pack} do
    node = Node.new(id: "exec-0", ast_origin: "exec-0", kind: :exec, inputs: %{}, state: :pending)
    assert {:error, :missing_exec_script, nil} = ExecRunner.run(node, %{run_id: "r", attempt: 1, pack_dir: pack})
  end

  test "a script that overruns its timeout is killed and reported", %{pack: pack} do
    rel = write_script!(pack, "scripts/slow.sh", "#!/bin/sh\nsleep 30\n")

    assert {:error, {:exec_timeout, 1, _output}, nil} =
             ExecRunner.run(exec_node(rel, timeout: 1), %{run_id: "r", attempt: 1, pack_dir: pack})
  end

  test "resolved DSL inputs reach the script as SYMPHONY_INPUT_* env vars", %{pack: pack} do
    rel =
      write_script!(pack, "scripts/env.sh", """
      #!/bin/sh
      echo "prefix=${SYMPHONY_INPUT_TITLE_PREFIX}"
      echo "days=${SYMPHONY_INPUT_LOOKBACK_DAYS}"
      echo "flag=${SYMPHONY_INPUT_DRY_RUN}"
      """)

    run_opts = %{
      run_id: "r",
      attempt: 1,
      pack_dir: pack,
      resolved_inputs: %{
        "title_prefix" => "idiomatic:",
        "lookback_days" => 30,
        "dry_run" => true,
        # Runner-reserved keys must not leak into the script env.
        "script" => rel,
        "timeout" => 5
      }
    }

    assert {:ok, %{exit_code: 0, output: output}, nil} = ExecRunner.run(exec_node(rel), run_opts)
    assert output =~ "prefix=idiomatic:"
    assert output =~ "days=30"
    assert output =~ "flag=true"
  end

  test "a JSON document written to SYMPHONY_OUTPUT_FILE becomes the structured node output", %{pack: pack} do
    rel =
      write_script!(pack, "scripts/structured.sh", """
      #!/bin/sh
      echo "gate log line" >&2
      printf '{"proceed": false, "blocking": [4711]}' > "$SYMPHONY_OUTPUT_FILE"
      """)

    assert {:ok, %{kind: :exec, exit_code: 0, output: output, log: log}, nil} =
             ExecRunner.run(exec_node(rel), %{run_id: "r", attempt: 1, pack_dir: pack})

    assert output == %{"proceed" => false, "blocking" => [4711]}
    assert log =~ "gate log line"
  end

  test "a script that writes nothing to SYMPHONY_OUTPUT_FILE keeps the stream tail as output", %{pack: pack} do
    rel = write_script!(pack, "scripts/plain-out.sh", "#!/bin/sh\necho only streams\n")

    assert {:ok, %{exit_code: 0, output: output} = output_map, nil} =
             ExecRunner.run(exec_node(rel), %{run_id: "r", attempt: 1, pack_dir: pack})

    assert output =~ "only streams"
    refute Map.has_key?(output_map, :log)
  end

  test "an invalid JSON result file fails the node loudly", %{pack: pack} do
    rel =
      write_script!(pack, "scripts/broken.sh", """
      #!/bin/sh
      printf 'not json' > "$SYMPHONY_OUTPUT_FILE"
      """)

    assert {:error, {:exec_output_invalid_json, _message, "not json"}, nil} =
             ExecRunner.run(exec_node(rel), %{run_id: "r", attempt: 1, pack_dir: pack})
  end

  test "the output file is removed after the attempt", %{pack: pack} do
    rel =
      write_script!(pack, "scripts/leave-file.sh", """
      #!/bin/sh
      printf '{"proceed": true}' > "$SYMPHONY_OUTPUT_FILE"
      echo "$SYMPHONY_OUTPUT_FILE" > result_path
      """)

    assert {:ok, %{output: %{"proceed" => true}}, nil} =
             ExecRunner.run(exec_node(rel), %{run_id: "r", attempt: 1, pack_dir: pack})

    result_path = pack |> Path.join("result_path") |> File.read!() |> String.trim()
    refute File.exists?(result_path)
  end
end
