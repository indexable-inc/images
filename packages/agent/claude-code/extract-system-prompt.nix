# Maintainer-facing tool that prints the Claude Code system prompt as actually
# sent, and can diff the stock upstream prompt against this package's overridden
# one. It works without binary disassembly or TLS interception: Claude Code
# honors `ANTHROPIC_BASE_URL`, so the script points a binary at a throwaway
# localhost server, runs it once in print mode from a clean HOME/cwd, and dumps
# the exact `system` blocks (and tool schemas) it transmits. The CLI assembles
# the prompt itself, so the result is faithful rather than reconstructed from
# fragments. See extract-system-prompt.py for the full rationale.
#
#   nix run .#claude-code.extractSystemPrompt                  # stock prompt text
#   nix run .#claude-code.extractSystemPrompt -- --mode wrapped  # house-overridden
#   nix run .#claude-code.extractSystemPrompt -- --mode diff     # stock vs wrapped
#   nix run .#claude-code.extractSystemPrompt -- --json          # {model,system,tools}
{
  ix,
  pkgs,
  name ? "claude-code-extract-system-prompt",
  # The unwrapped upstream binary: this package's libexec helper, the stock
  # download (autopatchelfed on Linux) with NO baked --system-prompt-file, MCP
  # config, or settings.
  stockBinary,
  # The wrapped launcher (`bin/claude`): the config-launch wrapper that applies
  # the house --system-prompt-file (full prompt replacement), --mcp-config, and
  # --settings. Probing this shows what our daily-driver prompt collapses to.
  wrappedBinary ? "claude",
}:
ix.writePythonApplication pkgs {
  inherit name;
  src = ./extract-system-prompt.py;
  # Bake both probe targets as defaults; a user-supplied --stock-binary /
  # --wrapped-binary on the CLI lands later in argv and overrides them.
  args = [
    "--stock-binary"
    stockBinary
    "--wrapped-binary"
    wrappedBinary
  ];
  meta.description = "Capture and diff the stock vs house-overridden Claude Code system prompt via a local ANTHROPIC_BASE_URL server";
}
