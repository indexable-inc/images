defmodule SymphonyElixir.Runtime.ExecRunner do
  @moduledoc """
  Executor for `:exec` IR nodes: one pack shell script, run in the pack
  directory. It lives outside the engine path because an exec node is not
  an engine turn.

  The node carries its script path and optional timeout as literal inputs
  (`inputs["script"]`, `inputs["timeout"]`), set by the interpreter from
  `exec "<path>" [timeout <seconds>] [{ ... }]`. The path is resolved
  relative to the active pack directory so a pack references its own
  scripts without carrying absolute deployment paths.

  Declared inputs reach the script as environment variables: each key of
  `run_opts.resolved_inputs` (the runtime resolves `{ name: value }` DSL
  inputs, including `${node.path}` references, before the attempt) is
  exported as `SYMPHONY_INPUT_<UPCASED_NAME>`. String values pass through
  verbatim; everything else is JSON-encoded so a script never has to guess
  the encoding.

  Structured results: the runner exports `SYMPHONY_OUTPUT_FILE` pointing at
  a fresh temp file. A script that writes a JSON document there gets that
  decoded value as its node output, which is what makes `when
  ${node.output.field}` gating and `map ${node.output.items}` fan-out work
  over exec nodes. A script that writes nothing keeps today's behavior (the
  combined stdout/stderr tail as `output`); a non-empty file that is not
  valid JSON fails the node loudly rather than guessing.

  The return shape matches `Runtime.EngineClient.run_node/2`
  (`{:ok, output, thread_id}` / `{:error, reason, thread_id}`) so the
  runtime treats every node kind through one result path; an exec has no
  engine thread, so `thread_id` is always `nil`.

  Bot identity: when a GitHub App is configured, a fresh installation
  token is injected as `GH_TOKEN` so `gh`/`git` inside the script author
  as the bot. A missing or unconfigured `GithubApp`/`Config` is not fatal:
  the script still runs with the inherited environment. This mirrors the
  conservative stance of the pre-overhaul exec path.
  """

  alias SymphonyElixir.{Config, GithubApp}
  alias SymphonyElixir.IR.Node

  require Logger

  # Keep the last 64 KB of combined stdout/stderr on the result: enough to
  # fingerprint a failure, small enough to keep the run file cheap.
  @output_tail_bytes 64 * 1024
  @default_timeout_seconds 300

  @type result :: {:ok, map(), nil} | {:error, term(), nil}

  @spec run(Node.t(), map()) :: result()
  def run(%Node{kind: :exec, inputs: inputs, id: node_id}, run_opts) when is_map(run_opts) do
    with {:ok, rel_path} <- fetch_script(inputs),
         pack_dir = pack_dir(run_opts),
         absolute = Path.expand(rel_path, pack_dir),
         :ok <- check_exists(absolute, rel_path),
         :ok <- check_executable(absolute, rel_path) do
      run_id = Map.get(run_opts, :run_id)
      timeout_seconds = fetch_timeout(inputs)
      output_file = output_file_path(run_id, node_id)
      Logger.info("ExecRunner run=#{run_id} node=#{node_id} cmd=#{rel_path} timeout=#{timeout_seconds}s")

      env =
        exec_env_with_bot_identity() ++
          input_env(run_opts) ++
          [{~c"SYMPHONY_OUTPUT_FILE", String.to_charlist(output_file)}]

      port =
        Port.open({:spawn_executable, absolute}, [
          :exit_status,
          :binary,
          :stderr_to_stdout,
          {:cd, pack_dir},
          {:env, env}
        ])

      deadline = System.monotonic_time(:millisecond) + timeout_seconds * 1_000

      try do
        port
        |> collect([], 0, deadline, run_id, node_id, timeout_seconds)
        |> apply_structured_output(output_file)
      after
        File.rm(output_file)
      end
    else
      {:error, reason} -> {:error, reason, nil}
    end
  end

  defp fetch_script(inputs) do
    case Map.get(inputs, "script") do
      {:literal, path} when is_binary(path) and path != "" -> {:ok, path}
      _ -> {:error, :missing_exec_script}
    end
  end

  # An exec timeout is optional in the surface; default to a finite bound
  # so a runaway script eventually fails the run rather than hanging.
  defp fetch_timeout(inputs) do
    case Map.get(inputs, "timeout") do
      {:literal, n} when is_integer(n) and n > 0 -> n
      _ -> @default_timeout_seconds
    end
  end

  defp pack_dir(run_opts) do
    case Map.get(run_opts, :pack_dir) do
      dir when is_binary(dir) and dir != "" -> dir
      _ -> Config.get().pack_dir
    end
  end

  # One fresh file per attempt: a retry must not read a stale result from
  # the previous attempt, so the path carries a unique integer.
  defp output_file_path(run_id, node_id) do
    unique = System.unique_integer([:positive])
    Path.join(System.tmp_dir!(), "symphony-exec-#{run_id}-#{node_id}-#{unique}.json")
  end

  # The DSL inputs the runtime resolved for this attempt, minus the two
  # runner-reserved keys, as SYMPHONY_INPUT_* environment entries. DSL
  # input names are identifiers, so upcasing yields a valid env var name.
  defp input_env(run_opts) do
    run_opts
    |> Map.get(:resolved_inputs, %{})
    |> Map.drop(["script", "timeout"])
    |> Enum.map(fn {name, value} ->
      {String.to_charlist("SYMPHONY_INPUT_" <> String.upcase(name)), String.to_charlist(env_value(value))}
    end)
  end

  defp env_value(value) when is_binary(value), do: value
  defp env_value(value), do: Jason.encode!(value)

  # A successful exec that wrote SYMPHONY_OUTPUT_FILE returns the decoded
  # JSON as its output (the stream tail moves to `log`); an empty or absent
  # file keeps the stream tail as output. A non-empty file that does not
  # decode is a loud failure: the script claimed a structured result and
  # broke the contract, so guessing would hide the defect.
  defp apply_structured_output({:ok, %{output: log} = output_map, nil}, output_file) do
    case File.read(output_file) do
      {:ok, raw} when raw != "" ->
        case Jason.decode(raw) do
          {:ok, decoded} ->
            {:ok, Map.put(%{output_map | output: decoded}, :log, log), nil}

          {:error, decode_error} ->
            {:error, {:exec_output_invalid_json, Exception.message(decode_error), raw}, nil}
        end

      _ ->
        {:ok, output_map, nil}
    end
  end

  defp apply_structured_output(result, _output_file), do: result

  defp check_exists(absolute, rel_path) do
    if File.exists?(absolute), do: :ok, else: {:error, {:exec_not_found, rel_path}}
  end

  # POSIX execute bit on owner/group/other. Surfacing a clear error beats
  # letting Port.open crash with EACCES.
  defp check_executable(absolute, rel_path) do
    case File.stat(absolute) do
      {:ok, %File.Stat{mode: mode}} ->
        if Bitwise.band(mode, 0o111) != 0, do: :ok, else: {:error, {:exec_not_executable, rel_path}}

      _ ->
        {:error, {:exec_not_executable, rel_path}}
    end
  end

  defp collect(port, acc, acc_bytes, deadline, run_id, node_id, timeout_seconds) do
    remaining = max(deadline - System.monotonic_time(:millisecond), 0)

    receive do
      {^port, {:data, chunk}} when is_binary(chunk) ->
        Logger.info("[exec #{run_id}/#{node_id}] " <> String.trim_trailing(chunk))
        {next_acc, next_bytes} = append_with_tail(acc, acc_bytes, chunk)
        collect(port, next_acc, next_bytes, deadline, run_id, node_id, timeout_seconds)

      {^port, {:exit_status, 0}} ->
        {:ok, %{kind: :exec, exit_code: 0, output: IO.iodata_to_binary(acc)}, nil}

      {^port, {:exit_status, status}} ->
        {:error, {:exec_failed, status, IO.iodata_to_binary(acc)}, nil}
    after
      remaining ->
        # :spawn_executable does not die on Port.close, so kill the OS
        # process explicitly, then drain the close so the mailbox stays clean.
        case Port.info(port, :os_pid) do
          {:os_pid, os_pid} ->
            Logger.warning("ExecRunner timeout run=#{run_id} node=#{node_id} after #{timeout_seconds}s; killing pid=#{os_pid}")
            System.cmd("kill", ["-KILL", Integer.to_string(os_pid)], stderr_to_stdout: true)

          _ ->
            :ok
        end

        Port.close(port)
        {:error, {:exec_timeout, timeout_seconds, IO.iodata_to_binary(acc)}, nil}
    end
  end

  # iodata accumulator with a byte budget: keep the tail, drop the head.
  defp append_with_tail(acc, acc_bytes, chunk) do
    chunk_size = byte_size(chunk)
    combined = acc_bytes + chunk_size

    cond do
      combined <= @output_tail_bytes ->
        {[acc, chunk], combined}

      chunk_size >= @output_tail_bytes ->
        {binary_part(chunk, chunk_size - @output_tail_bytes, @output_tail_bytes), @output_tail_bytes}

      true ->
        joined = IO.iodata_to_binary([acc, chunk])
        drop = byte_size(joined) - @output_tail_bytes
        {binary_part(joined, drop, @output_tail_bytes), @output_tail_bytes}
    end
  end

  # Inherit the BEAM env (PATH for gh/jq/git, secrets from the unit's
  # EnvironmentFile), then append a fresh GitHub App token as GH_TOKEN when
  # one is configured. gh prefers GH_TOKEN over GITHUB_TOKEN, so appending
  # is enough; the inherited PAT need not be scrubbed.
  defp exec_env_with_bot_identity do
    base = inherited_env()

    # Best-effort: no token means the script runs with the inherited env only.
    case GithubApp.best_effort_installation_token() do
      {:ok, token} -> base ++ [{~c"GH_TOKEN", String.to_charlist(token)}]
      :none -> base
    end
  end

  defp inherited_env do
    System.get_env()
    |> Enum.map(fn {k, v} -> {String.to_charlist(k), String.to_charlist(v)} end)
  end
end
