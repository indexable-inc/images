# Canonical model table for the pi-harnesses collection.
#
# Each alias maps to the Pi provider name and model id passed straight through
# to `pi --provider <provider> --model <model>`. API keys are NOT stored here:
# the harness receives them from the caller's environment and Pi reads the named
# env var itself. This keeps the runtime pure - it owns model selection, not
# secret lookup. Mirrors packages/pi-harnesses/engine/models.nix; the engine
# keeps its own copy until the two converge.
{
  # Executor-class models.
  claude = {
    provider = "anthropic";
    model = "claude-opus-4-8";
    apiKeyEnv = "ANTHROPIC_API_KEY";
  };

  # gpt-5.5 at medium reasoning effort. `thinking` is passed through as
  # `pi --thinking medium`. (opus-4-8 takes no thinking level: on 4.8 adaptive
  # thinking is the only mode, so `claude` omits it.)
  codex = {
    provider = "openai";
    model = "gpt-5.5";
    thinking = "medium";
    apiKeyEnv = "OPENAI_API_KEY";
  };
}
