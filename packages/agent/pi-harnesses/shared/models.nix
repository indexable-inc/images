# Canonical model table for the pi-harnesses collection.
#
# Each alias maps to the Pi provider name and model id passed straight through
# to `pi --provider <provider> --model <model>`. API keys are NOT stored here:
# the harness receives them from the caller's environment and Pi reads the named
# env var itself. This keeps the runtime pure - it owns model selection, not
# secret lookup. Mirrors packages/agent/pi-harnesses/engine/models.nix; the engine
# keeps its own copy until the two converge.
{
  # Executor-class models.
  claude = {
    provider = "anthropic";
    model = "claude-opus-4-8";
    apiKeyEnv = "ANTHROPIC_API_KEY";
  };

  # gpt-5.6-sol at medium reasoning effort. `thinking` is passed through as
  # `pi --thinking medium`. (opus-4-8 takes no thinking level: on 4.8 adaptive
  # thinking is the only mode, so `claude` omits it.)
  codex = {
    provider = "openai";
    model = "gpt-5.6-sol";
    thinking = "medium";
    apiKeyEnv = "OPENAI_API_KEY";
  };

  # Fable primary for fusion-style harnesses. Kept as a normal alias so model
  # availability can move without changing harness logic.
  fable = {
    provider = "anthropic";
    model = "fable-5";
    apiKeyEnv = "ANTHROPIC_API_KEY";
  };

  # Kimi K3 via Moonshot's OpenAI-compatible API. The pinned pi build has no
  # built-in kimi-k3 provider, so this alias carries a provider extension that
  # registers one (see providers/moonshot.js). No `thinking`: K3 only accepts
  # reasoning_effort=max and the extension pins every level to it.
  kimi = {
    provider = "moonshot";
    model = "kimi-k3";
    apiKeyEnv = "MOONSHOT_API_KEY";
    providerExtension = ./providers/moonshot.js;
  };

  # Cheap delegated worker for fusion-style harnesses.
  codex-low = {
    provider = "openai";
    model = "gpt-5.6-sol";
    thinking = "low";
    apiKeyEnv = "OPENAI_API_KEY";
  };
}
