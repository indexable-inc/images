defmodule SymphonyElixir.Engine.EnvelopeTest do
  use ExUnit.Case, async: true

  alias SymphonyElixir.Engine.Envelope

  describe "from_map/1" do
    test "builds a valid codex envelope and defaults permissions and location" do
      assert {:ok, env} =
               Envelope.from_map(%{"engine" => "codex", "model" => "gpt-5.3-codex", "effort" => "medium"})

      assert env.engine == :codex
      assert env.model == "gpt-5.3-codex"
      assert env.effort == :medium
      assert env.permissions == :workspace_write
      assert env.location == :local
    end

    test "builds a valid claude envelope" do
      assert {:ok, env} =
               Envelope.from_map(%{
                 "engine" => "claude",
                 "model" => "claude-opus-4-8",
                 "permissions" => "danger_full_access",
                 "location" => "local"
               })

      assert env.engine == :claude
      assert env.permissions == :danger_full_access
    end

    test "parses host and room locations" do
      assert {:ok, %{location: {:host, "hari"}}} =
               Envelope.from_map(%{"engine" => "codex", "model" => "gpt-5.3-codex", "location" => %{"host" => "hari"}})

      assert {:ok, %{location: {:room, "https://r"}}} =
               Envelope.from_map(%{"engine" => "codex", "model" => "gpt-5.3-codex", "location" => %{"room" => "https://r"}})
    end

    test "rejects a claude model under engine: codex" do
      assert {:error, {:engine_model_mismatch, :codex, "opus"}} =
               Envelope.from_map(%{"engine" => "codex", "model" => "opus"})
    end

    test "rejects a non-claude model under engine: claude" do
      assert {:error, {:engine_model_mismatch, :claude, "gpt-5.3-codex"}} =
               Envelope.from_map(%{"engine" => "claude", "model" => "gpt-5.3-codex"})
    end

    test "rejects unknown keys instead of silently ignoring them" do
      assert {:error, {:unknown_envelope_keys, ["sandbox"]}} =
               Envelope.from_map(%{"engine" => "claude", "model" => "opus", "sandbox" => "workspace-write"})
    end

    test "rejects an out-of-range effort" do
      assert {:error, {:invalid_effort, "ultra"}} =
               Envelope.from_map(%{"engine" => "codex", "model" => "gpt-5.3-codex", "effort" => "ultra"})
    end

    test "requires engine and model" do
      assert {:error, {:missing_envelope_field, "engine"}} = Envelope.from_map(%{"model" => "opus"})
      assert {:error, {:missing_envelope_field, "model"}} = Envelope.from_map(%{"engine" => "claude"})
    end
  end

  describe "claude_model?/1" do
    test "matches claude prefixes and aliases" do
      assert Envelope.claude_model?("claude-opus-4-8")
      assert Envelope.claude_model?("opus")
      assert Envelope.claude_model?("SONNET")
      refute Envelope.claude_model?("gpt-5.3-codex")
    end
  end
end
