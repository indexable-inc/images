# claude-code-rainbow-live: launch Claude Code with a LOOPING rainbow animation,
# injected at runtime over the Bun inspector (no binary edits, no recompile).
#
# Byte-patching can only swap static color constants (one key = one flat color)
# and cannot animate. This attaches to the stock binary's inspector and evaluates
# a payload that hooks process.stdout.write to rotate truecolor escapes over time,
# re-emitting the last captured frame on a timer (Ink diffs identical frames, so a
# timer alone will not repaint an idle screen; replaying the frame does).
{
  ix,
  repoPackages ? {},
}: let
  claude-code =
    repoPackages.claude-code
      or (throw "claude-code-rainbow-live: needs the claude-code sibling (flake package set only)");
  claudeBin = "${claude-code}/libexec/Claude Code";
in
  ix.writePythonApplication ix.pkgs {
    name = "claude-rainbow";
    src = ./claude-rainbow.py;
    # Bake the stock binary as the launch target; a user --claude-bin still wins.
    args = ["--claude-bin" claudeBin];
    meta.description = "Claude Code with a runtime-injected looping rainbow animation";
  }
