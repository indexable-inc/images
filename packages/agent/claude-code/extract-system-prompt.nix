# Maintainer-facing tool that prints the STOCK Claude Code system prompt, with
# none of this package's house overrides applied. It works without binary
# disassembly or TLS interception: Claude Code honors `ANTHROPIC_BASE_URL`, so
# the script points the unwrapped upstream binary at a throwaway localhost
# server, runs it once in print mode from a clean HOME/cwd, and dumps the exact
# `system` blocks (and tool schemas) the binary transmits. The CLI assembles the
# prompt itself, so the result is faithful rather than reconstructed from
# fragments. See extract-system-prompt.py for the full rationale.
#
#   nix run .#claude-code.extractSystemPrompt              # readable prompt text
#   nix run .#claude-code.extractSystemPrompt -- --json    # {model, system, tools}
#   nix run .#claude-code.extractSystemPrompt -- --tools   # prompt + tool schemas
{
  ix,
  pkgs,
  # The unwrapped upstream binary: this package's libexec helper, which is the
  # stock download (autopatchelfed on Linux) with NO baked
  # --append-system-prompt-file, MCP config, or settings. Running the
  # `$out/bin` wrapper instead would fold the house prompt into the capture.
  claudeBinary,
}:
ix.writePythonApplication pkgs {
  name = "claude-code-extract-system-prompt";
  src = ./extract-system-prompt.py;
  # Bake the stock binary as the default probe target; a user-supplied
  # `--claude-binary` on the CLI lands later in argv and overrides it.
  args = [
    "--claude-binary"
    claudeBinary
  ];
  meta.description = "Capture the stock Claude Code system prompt via a local ANTHROPIC_BASE_URL server";
}
