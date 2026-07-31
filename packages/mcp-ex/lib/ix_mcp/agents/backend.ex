defmodule IxMcp.Agents.Backend do
  @moduledoc """
  Pure argv/env builders for the subagent CLI backends; the runner spawns
  what these return, and keeping them pure keeps the lockdown testable.

  Lockdown per backend, structural and belt-and-suspenders:

    * claude/kimi: `--strict-mcp-config` with either an empty server set (the
      default: no kernel, so no spawn surface at all below the child) or, for
      `kernel: true`, exactly one server -- a kernel of its own. Plus
      `--disallowedTools Agent,Task` and
      CLAUDE_AGENT_SDK_DISABLE_BUILTIN_AGENTS=1, at every level: fan-out goes
      through `IxMcp.Agents`, so the native subagent tools stay off whether or
      not the child can spawn. The tool deny mirrors `defaultSystemTools` in
      packages/claude-code/default.nix; keep the two in sync.
    * codex: `-c agents.max_depth=1` (the CLI minimum; 1 is the thread
      itself, no children) and `-c mcp_servers={}`. No kernel option: codex
      has no stdin channel to steer, so a nesting codex child would be a tree
      nobody can reach into.
    * all: IX_AGENT_DEPTH, which `IxMcp.Agents.spawn/2` checks against
      `max_depth/0` before spawning, so the tree is bounded by construction
      rather than by the child's good behaviour (index#3700, index#4486).

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

  # Mirrors packages/claude-code/default.nix defaultSystemTools
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

    # Refused rather than ignored: a codex child has no stdin channel, so a
    # kernel-bearing one could spawn a subtree that neither its parent nor
    # anyone else can steer or interrupt.
    if Keyword.get(opts, :kernel, false) do
      raise ArgumentError,
            "kernel: true is unsupported for the codex backend: `codex exec` has no stdin " <>
              "channel, so a nesting codex child would be unsteerable. Use :claude or :kimi."
    end

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

    %{exe: exe(opts, "codex"), args: args, env: child_env(opts, []), stdin: :closed}
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
        mcp_config(opts),
        "--disallowedTools",
        @claude_deny,
        "--permission-mode",
        Keyword.get(opts, :permission_mode, "acceptEdits")
      ] ++
        model_args(Keyword.get(opts, :model)) ++
        resume_args(Keyword.get(opts, :resume)) ++
        allowed_args(Keyword.get(opts, :allowed_tools))

    %{exe: exe(opts, "claude"), args: args, env: child_env(opts, extra_env), stdin: :stream}
  end

  # :default means "the CLI's own configured default model" (codex).
  defp model_args(:default), do: []
  defp model_args(nil), do: []
  defp model_args(model) when is_binary(model), do: ["--model", model]

  defp resume_args(nil), do: []
  defp resume_args(session_id), do: ["--resume", session_id]

  defp allowed_args(nil), do: []
  defp allowed_args(tools) when is_list(tools), do: ["--allowedTools", Enum.join(tools, ",")]

  # A child with no kernel of its own still carries its depth: the CLI passes
  # its environment to any MCP server it starts, so an operator-configured
  # kernel (one this module did not write the config for) lands at the right
  # depth rather than looking like a lead.
  defp child_env(opts, extra) do
    [
      {~c"IX_AGENT_DEPTH", depth_charlist(opts)},
      {~c"CLAUDE_AGENT_SDK_DISABLE_BUILTIN_AGENTS", ~c"1"}
    ] ++ extra
  end

  # An empty server set unless the spawn asked for a kernel. `:kernel` is a
  # per-spawn opt-in rather than the default because a kernel-bearing child is
  # the one thing that makes the tree deeper than the measured shape
  # (index#3700's depth-1 star).
  defp mcp_config(opts) do
    if Keyword.get(opts, :kernel, false) do
      kernel_mcp_config(opts)
    else
      @empty_mcp
    end
  end

  # Named `index`, the same name the parent's kernel has, deliberately: this is
  # a separate OS process with `--strict-mcp-config`, so there is no parent
  # connection for the name to collide with, and the child's prompt and skills
  # already speak about "the index kernel". The #2382 collision is a different
  # shape -- an INLINE server in a Claude Code subagent definition, which shares
  # the parent session's connection and is silently dropped when the name
  # matches.
  defp kernel_mcp_config(opts) do
    JSON.encode!(%{
      "mcpServers" => %{
        "index" => %{
          "command" => kernel_bin(),
          "args" => [],
          "env" => %{
            "IX_AGENT_DEPTH" => Integer.to_string(Keyword.fetch!(opts, :depth)),
            "IX_AGENT_ID" => Keyword.fetch!(opts, :agent_id),
            "IX_AGENT_PARENT_SESSION" => Integer.to_string(Keyword.fetch!(opts, :parent_session))
          }
        }
      }
    })
  end

  # The nix wrapper bakes IX_MCP_EX_BIN to its own store path (the IX_MCP_GH
  # pattern), so a child kernel is the same build as the parent rather than
  # whatever PATH the MCP client happened to launch with.
  defp kernel_bin do
    System.get_env("IX_MCP_EX_BIN") ||
      System.find_executable("ix-mcp-ex") ||
      raise "a kernel-bearing child needs the mcp-ex binary: IX_MCP_EX_BIN is unset and " <>
              "ix-mcp-ex is not on PATH (the nix wrapper bakes it; a mix run must set it)"
  end

  defp depth_charlist(opts) do
    opts |> Keyword.fetch!(:depth) |> Integer.to_string() |> String.to_charlist()
  end

  # The :bin override exists for tests (stub scripts) and future remote
  # spawns where the path differs per host.
  defp exe(opts, default) do
    Keyword.get(opts, :bin) ||
      System.find_executable(default) ||
      raise "#{default} not found on PATH"
  end
end
