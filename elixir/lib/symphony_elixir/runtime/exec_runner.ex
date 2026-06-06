defmodule SymphonyElixir.Runtime.ExecRunner do
  @moduledoc """
  Executor for `:exec` IR nodes: one pack shell script, run in the pack
  directory. It lives outside the engine path because an exec node is not
  an engine turn.

  The node carries its script path and optional timeout as literal inputs
  (`inputs["script"]`, `inputs["timeout"]`), set by the interpreter from
  `exec "<path>" [timeout <seconds>]`. The path is resolved relative to
  the active pack directory so a pack references its own scripts without
  carrying absolute deployment paths.

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

  require Logger

  alias SymphonyElixir.{Config, GithubApp}
  alias SymphonyElixir.IR.Node

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
      Logger.info("ExecRunner run=#{run_id} node=#{node_id} cmd=#{rel_path} timeout=#{timeout_seconds}s")

      port =
        Port.open({:spawn_executable, absolute}, [
          :exit_status,
          :binary,
          :stderr_to_stdout,
          {:cd, pack_dir},
          {:env, exec_env_with_bot_identity()}
        ])

      deadline = System.monotonic_time(:millisecond) + timeout_seconds * 1_000
      collect(port, [], 0, deadline, run_id, node_id, timeout_seconds)
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

    case bot_token() do
      {:ok, token} -> base ++ [{~c"GH_TOKEN", String.to_charlist(token)}]
      :none -> base
    end
  end

  defp inherited_env do
    System.get_env()
    |> Enum.map(fn {k, v} -> {String.to_charlist(k), String.to_charlist(v)} end)
  end

  # Best-effort: a missing Config/GithubApp process (tests, dev) yields no
  # token rather than crashing the script run.
  defp bot_token do
    if GithubApp.configured?() do
      case GithubApp.installation_token() do
        {:ok, token} ->
          {:ok, token}

        {:error, reason} ->
          Logger.warning("ExecRunner: GitHub App token mint failed (#{inspect(reason)}); script runs with inherited env only")
          :none
      end
    else
      :none
    end
  rescue
    error ->
      Logger.warning("ExecRunner: bot identity unavailable (#{inspect(error)}); script runs with inherited env only")
      :none
  end
end
