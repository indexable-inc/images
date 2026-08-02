defmodule IxMcp.Agents.BackendTest do
  # async: false because the kimi cases mutate MOONSHOT_API_KEY.
  use ExUnit.Case, async: false

  alias IxMcp.Agents.Backend

  defp value_of(args, flag) do
    case Enum.find_index(args, &(&1 == flag)) do
      nil -> nil
      i -> Enum.at(args, i + 1)
    end
  end

  test "claude spec locks down MCP and built-in subagents, streams stdin" do
    spec = Backend.command(:claude, bin: "/bin/echo", model: "sonnet", prompt: "the brief")

    assert spec.stdin == :stream
    assert spec.exe == "/bin/echo"
    assert "--strict-mcp-config" in spec.args
    assert value_of(spec.args, "--mcp-config") == ~S({"mcpServers":{}})
    assert value_of(spec.args, "--disallowedTools") == "Agent,Task"
    assert value_of(spec.args, "--model") == "sonnet"
    assert value_of(spec.args, "--permission-mode") == "acceptEdits"
    # The prompt rides stdin, never argv (variadic --disallowedTools would
    # swallow it).
    refute "the brief" in spec.args
    assert {~c"IX_AGENT_CHILD", ~c"1"} in spec.env
    assert {~c"CLAUDE_AGENT_SDK_DISABLE_BUILTIN_AGENTS", ~c"1"} in spec.env
  end

  test "claude resume and allowed tools" do
    spec =
      Backend.command(:claude,
        bin: "/bin/echo",
        model: "sonnet",
        resume: "sid-1",
        allowed_tools: ["Read", "Grep"]
      )

    assert value_of(spec.args, "--resume") == "sid-1"
    assert value_of(spec.args, "--allowedTools") == "Read,Grep"
  end

  test "claude launcher arguments precede the child CLI arguments" do
    spec =
      Backend.command(:claude,
        bin: "/bin/remote-claude",
        launcher_args: ["loom-a1", "/root/work"]
      )

    assert ["loom-a1", "/root/work", "-p" | _rest] = spec.args
  end

  test "kimi is claude pointed at moonshot; raises without the key" do
    System.delete_env("MOONSHOT_API_KEY")

    assert_raise RuntimeError, ~r/MOONSHOT_API_KEY/, fn ->
      Backend.command(:kimi, bin: "/bin/echo", model: "kimi-k3")
    end

    System.put_env("MOONSHOT_API_KEY", "test-key")
    on_exit(fn -> System.delete_env("MOONSHOT_API_KEY") end)

    spec = Backend.command(:kimi, bin: "/bin/echo", model: "kimi-k3")
    assert {~c"ANTHROPIC_BASE_URL", ~c"https://api.moonshot.ai/anthropic"} in spec.env
    assert {~c"ANTHROPIC_AUTH_TOKEN", ~c"test-key"} in spec.env
    assert value_of(spec.args, "--model") == "kimi-k3"
  end

  test "codex spec: stdin closed, depth capped, prompt last" do
    spec = Backend.command(:codex, bin: "/bin/echo", model: :default, prompt: "do it")

    assert spec.stdin == :closed
    assert ["exec", "--json" | _rest] = spec.args
    assert value_of(spec.args, "-c") == "agents.max_depth=1"
    assert "mcp_servers={}" in spec.args
    assert List.last(spec.args) == "do it"
    # :default means codex's own configured model: no --model flag.
    assert value_of(spec.args, "--model") == nil
  end

  test "codex resume threads through exec resume" do
    spec =
      Backend.command(:codex, bin: "/bin/echo", model: :default, prompt: "more", resume: "tid-9")

    assert ["exec", "resume", "tid-9", "--json" | _rest] = spec.args
  end
end
