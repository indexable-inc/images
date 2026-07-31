defmodule IxMcp.Agents.BackendTest do
  # async: false because the kimi and kernel cases mutate process env.
  use ExUnit.Case, async: false

  alias IxMcp.Agents.Backend

  defp value_of(args, flag) do
    case Enum.find_index(args, &(&1 == flag)) do
      nil -> nil
      i -> Enum.at(args, i + 1)
    end
  end

  defp claude!(opts) do
    Backend.command(:claude, Keyword.merge([bin: "/bin/echo", model: "sonnet", depth: 1], opts))
  end

  defp put_env(name, value) do
    System.put_env(name, value)
    on_exit(fn -> System.delete_env(name) end)
  end

  test "claude spec locks down MCP and built-in subagents, streams stdin" do
    spec = claude!(prompt: "the brief")

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
    assert {~c"IX_AGENT_DEPTH", ~c"1"} in spec.env
    assert {~c"CLAUDE_AGENT_SDK_DISABLE_BUILTIN_AGENTS", ~c"1"} in spec.env
  end

  test "claude resume and allowed tools" do
    spec = claude!(resume: "sid-1", allowed_tools: ["Read", "Grep"])

    assert value_of(spec.args, "--resume") == "sid-1"
    assert value_of(spec.args, "--allowedTools") == "Read,Grep"
  end

  describe "kernel-bearing children (#4486)" do
    test "one server, named index, carrying the child's place in the tree" do
      put_env("IX_MCP_EX_BIN", "/nix/store/fake-mcp-ex/bin/ix-mcp-ex")

      spec = claude!(kernel: true, depth: 2, agent_id: "sub-7", parent_session: 41)

      assert %{"mcpServers" => %{"index" => server}} =
               JSON.decode!(value_of(spec.args, "--mcp-config"))

      assert server["command"] == "/nix/store/fake-mcp-ex/bin/ix-mcp-ex"
      assert server["args"] == []

      assert server["env"] == %{
               "IX_AGENT_DEPTH" => "2",
               "IX_AGENT_ID" => "sub-7",
               "IX_AGENT_PARENT_SESSION" => "41"
             }

      # The lockdown that survives nesting: fan-out stays the kernel's job at
      # every level, so the native subagent tools are denied here too.
      assert value_of(spec.args, "--disallowedTools") == "Agent,Task"
      assert "--strict-mcp-config" in spec.args
    end

    test "a child kernel with no binary to run is refused, not silently empty" do
      System.delete_env("IX_MCP_EX_BIN")
      on_exit(fn -> System.delete_env("IX_MCP_EX_BIN") end)

      if System.find_executable("ix-mcp-ex") do
        # A dev shell with the binary on PATH: the fallback is the point here.
        assert %{"mcpServers" => %{"index" => _server}} =
                 JSON.decode!(
                   value_of(
                     claude!(kernel: true, agent_id: "sub-1", parent_session: 1).args,
                     "--mcp-config"
                   )
                 )
      else
        assert_raise RuntimeError, ~r/IX_MCP_EX_BIN is unset/, fn ->
          claude!(kernel: true, agent_id: "sub-1", parent_session: 1)
        end
      end
    end

    test "codex refuses a kernel rather than ignoring the option" do
      assert_raise ArgumentError, ~r/no stdin channel/, fn ->
        Backend.command(:codex,
          bin: "/bin/echo",
          model: :default,
          depth: 1,
          prompt: "do it",
          kernel: true
        )
      end
    end
  end

  test "kimi is claude pointed at moonshot; raises without the key" do
    System.delete_env("MOONSHOT_API_KEY")

    assert_raise RuntimeError, ~r/MOONSHOT_API_KEY/, fn ->
      Backend.command(:kimi, bin: "/bin/echo", model: "kimi-k3", depth: 1)
    end

    put_env("MOONSHOT_API_KEY", "test-key")

    spec = Backend.command(:kimi, bin: "/bin/echo", model: "kimi-k3", depth: 1)
    assert {~c"ANTHROPIC_BASE_URL", ~c"https://api.moonshot.ai/anthropic"} in spec.env
    assert {~c"ANTHROPIC_AUTH_TOKEN", ~c"test-key"} in spec.env
    assert value_of(spec.args, "--model") == "kimi-k3"
  end

  test "codex spec: stdin closed, depth capped, prompt last" do
    spec = Backend.command(:codex, bin: "/bin/echo", model: :default, depth: 1, prompt: "do it")

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
      Backend.command(:codex,
        bin: "/bin/echo",
        model: :default,
        depth: 1,
        prompt: "more",
        resume: "tid-9"
      )

    assert ["exec", "resume", "tid-9", "--json" | _rest] = spec.args
  end
end
