defmodule SymphonyElixir.Claude.CodeTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.Claude.Code
  alias SymphonyElixir.Config

  test "command pipes the prompt on stdin and reads prompt/model from env" do
    cmd = Code.command("claude")

    # Prompt arrives on stdin, never on argv, so a leading dash or argv
    # length limit cannot corrupt the invocation.
    assert cmd =~ ~s(printf '%s' "$SYMPHONY_CLAUDE_PROMPT" | claude)
    assert cmd =~ "--print"
    assert cmd =~ "--output-format json"
    assert cmd =~ "--dangerously-skip-permissions"
    assert cmd =~ ~s(--model "$SYMPHONY_CLAUDE_MODEL")
    refute cmd =~ "SYMPHONY_CLAUDE_PROMPT="
  end

  test "command honors an overridden claude executable" do
    assert Code.command("/opt/bin/claude") =~ "| /opt/bin/claude --print"
  end

  test "run errors without an Anthropic API key rather than spawning claude" do
    assert {:error, :anthropic_api_key_not_configured} =
             Code.run(File.cwd!(), "hello", %{},
               config: %Config{anthropic_api_key: nil},
               model: "claude-opus-4-8"
             )
  end
end
