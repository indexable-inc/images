# The three hooks are subcommands of one compiled binary (packages/claude-hooks)
# rather than hand-rolled shell scripts. Each fails OPEN and SILENT (any
# missing input, parse error, or kill-switch exits with no stdout: a noisy or
# broken hook is strictly worse than no hook). See that crate for the full
# design and its measured rationale:
#  - session-digest (SessionStart): cat the pre-rendered fleet context digest
#    (`~/.cache/ix/context-digest.md`, ENG-2708), capped ~1500 tokens. Kill
#    switch: CLAUDE_CODE_DISABLE_CONTEXT_DIGEST=1.
#  - worktree-guard (PreToolUse): deny edits whose TARGET path resolves into a
#    protected primary checkout, judging the target not the session (ENG-2692).
#    Kill switch: CLAUDE_CODE_DISABLE_WORKTREE_GUARD=1.
#  - prompt-priors (UserPromptSubmit): triple-gated, score-gated ambient priors
#    from the corpus store (ENG-2707), capped ~1200 tokens. Kill switch:
#    CLAUDE_CODE_DISABLE_PROMPT_PRIORS=1.
# Tool paths and the baked primary-checkout default ride as env on a thin
# makeBinaryWrapper so the hook is self-contained under any user PATH, while
# user knobs keep their CLAUDE_CODE_* names. IX_SEARCH (and the prompt-priors
# hook itself) is wired only when the `search` sibling is in scope: like the
# index MCP server it ships from the flake package set, not the overlay (see
# `repoPackages`).
{
  lib,
  runCommand,
  makeBinaryWrapper,
  ix,
  git,
  primaryCheckouts,
  repoPackages,
}:
runCommand "claude-hooks-wrapped" { nativeBuildInputs = [ makeBinaryWrapper ]; } ''
  makeBinaryWrapper ${ix.rustWorkspace.units.binaries."claude-hooks"}/bin/claude-hooks \
    $out/bin/claude-hooks \
    --set IX_GIT ${lib.getExe git} \
    --set IX_DEFAULT_PRIMARY_CHECKOUTS ${lib.escapeShellArg (lib.concatStringsSep ":" primaryCheckouts)} \
    ${lib.optionalString (repoPackages ? search) "--set IX_SEARCH ${lib.getExe repoPackages.search}"}
''
