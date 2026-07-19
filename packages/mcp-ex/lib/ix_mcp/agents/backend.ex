defmodule IxMcp.Agents.Backend do
  @moduledoc """
  Pure argv/env builders for the subagent CLI backends; the runner spawns
  what these return, and keeping them pure keeps the lockdown testable.

  Depth-1 lockdown per backend, structural and belt-and-suspenders:

    * claude/kimi: `--strict-mcp-config --mcp-config '{"mcpServers":{}}'`
      (no inherited MCP servers, so no kernel and no spawn surface below
      the child) plus `--disallowedTools Agent,Task` and
      CLAUDE_AGENT_SDK_DISABLE_BUILTIN_AGENTS=1 (no built-in subagent
      types). The tool deny mirrors `defaultSystemTools` in
      packages/agent/claude-code/default.nix; keep the two in sync.
    * codex: `-c agents.max_depth=1` (the CLI minimum; 1 is the thread
      itself, no children) and `-c mcp_servers={}`.
    * all: IX_AGENT_CHILD=1, which `IxMcp.Agents.spawn/2` raises under, so
      even a future kernel-bearing child cannot recurse (index#3700).

  Flag facts verified against the live CLIs (2026-07-19):

    * `-p --output-format stream-json` requires `--verbose`;
    * `--disallowedTools <tools...>` is variadic and swallows a trailing
      prompt, so the prompt never rides argv (claude reads it as a
      stream-json stdin line; codex takes it as the last argument);
    * `codex exec` blocks reading a piped stdin, so it must spawn with
      stdin closed (the runner wraps it in `sh -c 'exec "$0" "$@" </dev/null'`).
  """

  @type backend :: :claude | :codex | :kimi
  @type spec :: %{
          exe: String.t(),
          args: [String.t()],
          env: [{charlist(), charlist()}],
          stdin: :stream | :closed
        }

  # Mirrors packages/agent/claude-code/default.nix defaultSystemTools
  # (Agent = false, Task* = false); "Task" also denies the TaskCreate
  # family's umbrella tool name used by older harness builds.
  @claude_deny "Agent,Task"
  @empty_mcp ~S({"mcpServers":{}})

  @doc "Build the spawn spec for one working phase of a child."
  @spec command(backend(), keyword()) :: spec()
  def command(:claude, opts), do: claude(opts, [])

  def command(:kimi, opts) do
    key =
      System.get_env("MOONSHOT_API_KEY") ||
        raise "kimi backend needs MOONSHOT_API_KEY in the kernel's environment"

    claude(opts, [
      {~c"ANTHROPIC_BASE_URL", ~c"https://api.moonshot.ai/anthropic"},
      {~c"ANTHROPIC_AUTH_TOKEN", String.to_charlist(key)},
      {~c"ENABLE_TOOL_SEARCH", ~c"false"}
    ])
  end

  def command(:codex, opts) do
    prompt = Keyword.fetch!(opts, :prompt)

    subcommand =
      case Keyword.get(opts, :resume) do
        nil -> ["exec"]
        thread -> ["exec", "resume", thread]
      end

    args =
      subcommand ++
        ["--json", "--skip-git-repo-check"] ++
        ["-c", "agents.max_depth=1", "-c", "mcp_servers={}"] ++
        model_args(Keyword.get(opts, :model)) ++
        [prompt]

    %{exe: exe(opts, "codex"), args: args, env: child_env([]), stdin: :closed}
  end

  defp claude(opts, extra_env) do
    args =
      [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--strict-mcp-config",
        "--mcp-config",
        @empty_mcp,
        "--disallowedTools",
        @claude_deny,
        "--permission-mode",
        Keyword.get(opts, :permission_mode, "acceptEdits")
      ] ++
        model_args(Keyword.get(opts, :model)) ++
        resume_args(Keyword.get(opts, :resume)) ++
        allowed_args(Keyword.get(opts, :allowed_tools))

    %{exe: exe(opts, "claude"), args: args, env: child_env(extra_env), stdin: :stream}
  end

  # :default means "the CLI's own configured default model" (codex).
  defp model_args(:default), do: []
  defp model_args(nil), do: []
  defp model_args(model) when is_binary(model), do: ["--model", model]

  defp resume_args(nil), do: []
  defp resume_args(session_id), do: ["--resume", session_id]

  defp allowed_args(nil), do: []
  defp allowed_args(tools) when is_list(tools), do: ["--allowedTools", Enum.join(tools, ",")]

  defp child_env(extra) do
    [
      {~c"IX_AGENT_CHILD", ~c"1"},
      {~c"CLAUDE_AGENT_SDK_DISABLE_BUILTIN_AGENTS", ~c"1"}
    ] ++ extra
  end

  # The :bin override exists for tests (stub scripts) and future remote
  # spawns where the path differs per host.
  defp exe(opts, default) do
    Keyword.get(opts, :bin) ||
      System.find_executable(default) ||
      raise "#{default} not found on PATH"
  end
end
