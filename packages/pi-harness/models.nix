# Declarative model table for the Pi harness.
#
# Each alias maps to the Pi provider name and model id passed straight through to
# `pi --provider <provider> --model <model>`. API keys are NOT stored here: the
# harness receives them from the caller's environment (Room / deploy injects them
# from the ix secret-store per ENG-2261), and Pi reads the named env var itself.
# This keeps the runtime pure - it owns model selection, not secret lookup.
{
  # Claude Opus 4.8 via the Anthropic API. On 4.8 adaptive thinking is the only
  # thinking mode and temperature/top_p/budget_tokens are rejected, so the
  # harness never passes sampling params.
  claude = {
    provider = "anthropic";
    model = "claude-opus-4-8";
    apiKeyEnv = "ANTHROPIC_API_KEY";
  };

  # GPT-5.5 (the model behind Codex) via the OpenAI API. `gpt-5.5` is the API
  # model id - there is no separate `-codex` suffix on the API surface.
  codex = {
    provider = "openai";
    model = "gpt-5.5";
    apiKeyEnv = "OPENAI_API_KEY";
  };
}
