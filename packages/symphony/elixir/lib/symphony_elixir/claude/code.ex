defmodule SymphonyElixir.Claude.Code do
  @moduledoc """
  Runs one workflow node as a headless Claude Code session in the run's
  workspace, used when a skill's `codex_model` names a Claude model
  (`claude-*`, or the `opus` / `sonnet` / `haiku` aliases); every other
  model goes to Codex.

  This is the in-process Claude runner the YAML/DAG `NodeExecutor` used.
  The `.sym`/IR engine path runs Claude turns through the room-server's
  `engine_claude` adapter instead, so this module is not on the IR hot
  path. It is kept (not deleted with the rest of the old stack) because it
  is the only in-process Claude turn runner and removing it would orphan
  Claude support outside the room-server; revisit once the IR path proves
  Claude parity end to end on the room-server engine host.

  There is no app-server protocol, no VM, and no Symphony dynamic-tool
  surface here. This module spawns the `claude` CLI once, non-interactively,
  and reads back a single JSON result:

      printf '%s' "$prompt" | claude --print --output-format json \\
        --dangerously-skip-permissions --model claude-opus-4-8

  - `--print` runs Claude Code non-interactively and exits.
  - `--dangerously-skip-permissions` lets the agent edit files and run
    tools with no approval prompts. A Claude model is the opt-in for that;
    there is no per-tool gate the way codex has `approval_policy`.
  - `--output-format json` emits one result object on stdout whose
    `result`, `session_id`, and `is_error` fields we surface.
  - `--model` is the skill's `codex_model` value, passed through verbatim.

  The agent uses Claude Code's own tools (Bash, Edit, Read, ...) plus
  whatever CLIs are on PATH inside the workspace (`git`, `gh`). The
  GitHub App bot identity stamped into the checkout therefore applies to
  claude runs exactly as it does to codex runs.

  Auth is the Anthropic API key in `ANTHROPIC_API_KEY`, the same secret
  surface every other integration token flows through. It is injected
  into the subprocess env, never onto the command line, so it cannot
  leak into logs or run records.

  The prompt and model travel through the subprocess environment
  (`SYMPHONY_CLAUDE_PROMPT`, `SYMPHONY_CLAUDE_MODEL`) and the prompt is
  piped on stdin rather than passed positionally, so neither argv length
  limits nor a leading dash in the prompt can corrupt the command line.

  Bad fit if: `ANTHROPIC_API_KEY` is unset (the run errors with
  `:anthropic_api_key_not_configured`), or the Symphony service runs as
  root, where `--dangerously-skip-permissions` refuses to start.

  This engine ignores any placement: a Claude model run through this
  in-process path has no per-run room-server.
  """

  alias SymphonyElixir.Config

  require Logger

  @prompt_env "SYMPHONY_CLAUDE_PROMPT"
  @model_env "SYMPHONY_CLAUDE_MODEL"

  # One hour, matching the codex turn timeout. A workflow node that has
  # not produced its result JSON by then is treated as hung.
  @default_turn_timeout_ms 60 * 60 * 1000

  @type env_pair :: {String.t(), String.t()}
  @type context :: %{optional(:identifier) => String.t(), optional(:title) => String.t()}

  @spec run(Path.t(), String.t(), context(), keyword()) :: {:ok, map()} | {:error, term()}
  def run(workspace, prompt, _context, opts)
      when is_binary(workspace) and is_binary(prompt) and is_list(opts) do
    config = Keyword.fetch!(opts, :config)
    model = Keyword.fetch!(opts, :model)
    turn_timeout_ms = Keyword.get(opts, :turn_timeout_ms, @default_turn_timeout_ms)
    extra_env = Keyword.get(opts, :extra_env, [])

    with {:ok, api_key} <- fetch_api_key(config),
         {:ok, bash} <- find_bash(),
         :ok <- ensure_workspace(workspace) do
      env =
        env_charlists(
          extra_env ++
            [
              {"ANTHROPIC_API_KEY", api_key},
              {@prompt_env, prompt},
              {@model_env, model}
            ]
        )

      port =
        Port.open(
          {:spawn_executable, String.to_charlist(bash)},
          [
            :binary,
            :exit_status,
            args: [~c"-c", String.to_charlist(command(config.claude_command))],
            cd: String.to_charlist(workspace),
            env: env
          ]
        )

      deadline = System.monotonic_time(:millisecond) + turn_timeout_ms
      collect(port, deadline, turn_timeout_ms, [])
    end
  end

  # The command run under `bash -c`. The prompt and model are referenced
  # from the environment (double-quoted so the shell does not re-split or
  # glob them); piping the prompt on stdin keeps it off the argv entirely.
  @doc false
  @spec command(String.t()) :: String.t()
  def command(claude_command) when is_binary(claude_command) do
    "printf '%s' \"$#{@prompt_env}\" | " <>
      claude_command <>
      " --print --output-format json --dangerously-skip-permissions" <>
      " --model \"$#{@model_env}\""
  end

  defp fetch_api_key(%Config{anthropic_api_key: key}) when is_binary(key) and key != "", do: {:ok, key}
  defp fetch_api_key(%Config{}), do: {:error, :anthropic_api_key_not_configured}

  defp find_bash do
    case System.find_executable("bash") do
      nil -> {:error, :bash_not_found}
      bash -> {:ok, bash}
    end
  end

  defp ensure_workspace(workspace) do
    if File.dir?(workspace), do: :ok, else: {:error, {:workspace_not_directory, workspace}}
  end

  defp env_charlists(env) when is_list(env) do
    Enum.map(env, fn {k, v} when is_binary(k) and is_binary(v) ->
      {String.to_charlist(k), String.to_charlist(v)}
    end)
  end

  # Claude Code's json output format prints exactly one object on stdout
  # at the end of the turn, so we buffer everything and parse on exit
  # rather than streaming. stderr is left on the BEAM's stderr (no
  # :stderr_to_stdout) so progress and diagnostics reach journald without
  # polluting the JSON we have to decode.
  defp collect(port, deadline, timeout_ms, chunks) do
    remaining_ms = max(deadline - System.monotonic_time(:millisecond), 0)

    receive do
      {^port, {:data, data}} ->
        collect(port, deadline, timeout_ms, [data | chunks])

      {^port, {:exit_status, 0}} ->
        parse_result(output(chunks))

      {^port, {:exit_status, status}} ->
        {:error, {:claude_exit, status, tail(output(chunks))}}
    after
      remaining_ms ->
        kill_port(port)
        {:error, {:claude_turn_timeout, timeout_ms}}
    end
  end

  defp parse_result(stdout) do
    case Jason.decode(last_json_line(stdout)) do
      {:ok, %{"is_error" => false} = result} ->
        {:ok,
         %{
           kind: :claude,
           session_id: Map.get(result, "session_id"),
           result: Map.get(result, "result"),
           total_cost_usd: Map.get(result, "total_cost_usd")
         }}

      {:ok, %{"is_error" => true} = result} ->
        {:error, {:claude_turn_failed, Map.get(result, "subtype"), Map.get(result, "result")}}

      {:ok, other} ->
        {:error, {:claude_invalid_result, other}}

      {:error, _reason} ->
        {:error, {:claude_unparseable_output, tail(stdout)}}
    end
  end

  # Defensive: a stray non-JSON line on stdout (a tool that ignores the
  # json contract, a shell notice) should not mask the result object,
  # which json mode prints last. Take the last non-blank line.
  defp last_json_line(stdout) do
    stdout
    |> String.split("\n", trim: true)
    |> List.last()
    |> Kernel.||("")
  end

  defp output(chunks), do: chunks |> Enum.reverse() |> IO.iodata_to_binary()

  defp tail(text) when is_binary(text), do: String.slice(text, max(String.length(text) - 2_000, 0), 2_000)

  defp kill_port(port) do
    case Port.info(port, :os_pid) do
      {:os_pid, os_pid} -> System.cmd("kill", ["-KILL", Integer.to_string(os_pid)], stderr_to_stdout: true)
      _ -> :ok
    end

    if Port.info(port) != nil, do: Port.close(port)
    :ok
  rescue
    _ -> :ok
  end
end
