# Registry metadata. agent-harness-ex is the BEAM implementation of the
# Fable 5 system card's async-subagents multi-agent harness (sec 8.15.3,
# index#3700): agents as supervised processes with mailbox-backed
# Send Message / Wait for Message semantics. ix-mcp-ex consumes it as a mix
# path dependency; the flake output builds the compiled :agent_harness OTP
# app and `passthru.tests.elixir` gates the ExUnit/Credo lane.
{
  id = "agent-harness-ex";
  packageSet = true;
  flake = true;
  overlay = false;
  passthruTests = true;
}
