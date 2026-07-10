# claude-hooks

`packages/agent/claude-hooks` is one compiled binary with Claude Code hook
subcommands, replacing the old hand-rolled `writeShellScript` hooks in
`packages/agent/claude-code`. The governing rule: every hook fails OPEN and SILENT.
Any missing input, parse error, or kill-switch returns with no stdout, because a
noisy or broken hook is strictly worse than no hook
(`src/main.rs:1-10`).

```
claude-hooks session-digest    # SessionStart
claude-hooks worktree-guard    # PreToolUse
claude-hooks prompt-priors     # UserPromptSubmit
```

Dispatch is on `argv[1]` (`main`, `src/main.rs:66-77`); an unknown subcommand
prints to stderr and exits 2. Every other path exits 0. Output, when any, is a
single JSON line wrapping a `hookSpecificOutput` object (`Wrap<T>`/`emit`,
`src/main.rs:115-127`).

## Shared conventions

- **Kill switches.** A subcommand returns immediately if its
  `CLAUDE_CODE_DISABLE_*` env var is present and non-empty (`flag_set`,
  `src/main.rs:82-84`).
- **Env-injected tool paths.** The claude-code wrapper passes tool paths and the
  baked default via env: `IX_GIT`, `IX_SEARCH`, `IX_DEFAULT_PRIMARY_CHECKOUTS`
  (`src/main.rs:7-10`). User-facing knobs keep their `CLAUDE_CODE_*` names.
- **Char caps.** Injected context is truncated by char count, not bytes
  (`cap_chars`, `src/main.rs:96-98`).

## session-digest (SessionStart)

Reads `~/.cache/ix/context-digest.md`, caps it to `DIGEST_CAP = 6000` chars
(~1500 tokens, inside Claude Code's 10,000-char `additionalContext` limit), and
emits it as `additionalContext` with `hookEventName: "SessionStart"`
(`session_digest`, `src/main.rs:131-147`). Missing or empty file -> silent. Kill
switch: `CLAUDE_CODE_DISABLE_CONTEXT_DIGEST`. The digest itself is rendered
out-of-band (ENG-2708); this hook only cats it.

## worktree-guard (PreToolUse)

Denies a file-edit tool call whose TARGET path resolves into a protected primary
checkout, so the agent is pushed to work in a dedicated worktree
(`worktree_guard`, `src/main.rs:151-220`). Matcher in claude-code is
`Edit|MultiEdit|Write|NotebookEdit` (`packages/agent/claude-code/default.nix:258`).

Flow:

1. Read the `tool_input.file_path` (or `notebook_path`) from the hook stdin JSON
   (`src/main.rs:159-166`). Absent/empty -> silent allow.
2. Resolve the target: absolute path stands alone; a relative path resolves
   against the payload `cwd` (falling back to `PWD`, then `.`). It judges the
   target, never the session (`src/main.rs:168-180`).
3. Walk up to the nearest existing ancestor directory, since a new file's parent
   may not exist yet (`src/main.rs:182-191`).
4. Run `git -C <dir> rev-parse --path-format=absolute` for `--git-dir`,
   `--git-common-dir`, `--show-toplevel` using `IX_GIT` (`git_rev_parse`,
   `src/main.rs:222-239`). If the private git-dir differs from the common dir it
   is a linked worktree -> allow (`src/main.rs:200-203`).
5. If `--show-toplevel` matches a protected pattern, emit a `deny` with
   `hookEventName: "PreToolUse"`, `permissionDecision: "deny"`, and a reason that
   tells the agent to create a worktree (`src/main.rs:208-219`).

Protected patterns come from `CLAUDE_CODE_PRIMARY_CHECKOUTS` (user override) or
`IX_DEFAULT_PRIMARY_CHECKOUTS` (wrapper-baked), colon-separated, empties dropped;
an empty list disables the guard (`primary_checkouts`, `src/main.rs:243-251`).
Matching uses `glob::Pattern` with shell `case`-glob semantics where `*` crosses
`/` (`matches_protected`, `src/main.rs:255-259`). Kill switch:
`CLAUDE_CODE_DISABLE_WORKTREE_GUARD`. In claude-code the whole `PreToolUse` block
is only installed when `primaryCheckouts != []` (`default.nix:257`).

## prompt-priors (UserPromptSubmit)

Injects score-gated ambient priors from the corpus store, but only after passing
several cheap gates so it stays net-positive (`prompt_priors`,
`src/main.rs:263-289`). Kill switch: `CLAUDE_CODE_DISABLE_PROMPT_PRIORS`. Gates,
all must pass:

- **Word gate.** At least `MIN_WORDS = 8` whitespace tokens
  (`passes_word_gate`, `src/main.rs:291-293`): below this, ambient recall is
  measured net-negative.
- **Fleet-noun gate.** The prompt must contain a whole word from the
  `FLEET_NOUNS` allowlist (`src/main.rs:33-64`, `has_fleet_noun`); a prompt
  without one embeds near everything and pulls vendored-code noise. Case
  insensitive, whole-word (substring `reindexing` does not match `index`).
- **Credential gate.** `MXBAI_API_KEY` set or `~/.mgrep/token.json` exists
  (`has_credential`, `src/main.rs:302-304`).

If gated through, it runs `IX_SEARCH` with the prompt and
`--json --compact --no-rerank --max-count 3 --source
claude_history,shell,github` (`run_search`, `src/main.rs:306-349`) under a hard
2s budget (single-shot poll loop; kill on expiry). Hits are filtered to score >=
`SCORE_GATE = 0.70` and rendered with a stale/cross-user disclaimer header,
capped to `PRIORS_CAP = 4800` chars (~1200 tokens) (`render_priors`,
`src/main.rs:351-372`). Each hit's provenance line is `source [by user]
[timestamp] score N` (`provenance`, `src/main.rs:376-396`), matching the old jq
projection so the model can discount stale content. In claude-code this hook (and
`IX_SEARCH`) is wired only when the `search` sibling package is in scope
(`packages/agent/policy/hook-runner.nix:17-20,35`).

## How it is built and wired

`default.nix` selects the `claude-hooks` binary with
`ix.cargoUnit.selectBinaryWithTests` (flake output `claude-hooks`,
`package.nix`). The claude-code layer re-wraps it in
`packages/agent/policy/hook-runner.nix`: `makeBinaryWrapper` sets `IX_GIT`,
`IX_DEFAULT_PRIMARY_CHECKOUTS`, and (conditionally) `IX_SEARCH`, then registers
the subcommands as hook commands in the generated settings JSON
(`packages/agent/claude-code/default.nix:244-285`) with generous per-hook timeouts (5s,
10s, 5s) that sit well past the fail-open budgets. `install-check.nix` asserts
the fail-open behavior, the digest cap, and the guard deny/allow paths.

Unit tests (`src/main.rs:398-483`) cover the char cap, the word and fleet-noun
gates, the protected-glob slash-crossing, the priors score gate and cap, and the
provenance formatting.
