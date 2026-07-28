# claude-hooks

`packages/claude-hooks` is one compiled binary with Claude Code hook
subcommands, replacing the old hand-rolled `writeShellScript` hooks in
`packages/claude-code`. The governing rule: every hook fails OPEN and SILENT.
Any missing input, parse error, or kill-switch returns with no stdout, because a
noisy or broken hook is strictly worse than no hook
(`src/main.rs:1-10`).

```
claude-hooks session-digest    # SessionStart
claude-hooks worktree-guard    # PreToolUse
claude-hooks write-guard       # PreToolUse (Bash)
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
`Edit|MultiEdit|Write|NotebookEdit` (`packages/claude-code/default.nix:258`).

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

## git-guard (PreToolUse, Bash)

Refuses a git command that would mutate a shared primary checkout. Same
protected list as the worktree guard above, so it is only installed when
`primaryCheckouts != []`. Kill switch: `CLAUDE_CODE_DISABLE_GIT_GUARD`.

Three separate harms, in the order the guard reports them:

1. **Destroying uncommitted work** (ENG-9964): a subcommand from
   `discards_worktree` against a checkout whose `git status --porcelain` is
   non-empty.
2. **Desyncing the tree from its own HEAD** (index#4211): a pathspec `checkout`
   or `restore` reading a revision other than `HEAD`, which a clean tree does
   not protect against.
3. **Mutating a checkout that is not yours at all** (index#4218): any
   subcommand from `mutates_checkout`, whatever the state of the tree.

The third is the broad rule and the other two are refinements of it that say
more about what specifically goes wrong. It exists because the first two left a
hole big enough to walk a shared checkout through: `~/.config/nix/ix` was found
on a branch deleted upstream, 604 commits behind main, with 534 files staged by
nobody. No edit tool was involved, so `worktree-guard` (which judges edit-tool
target paths and never sees Bash) never fired; `git add` and `git switch` lose
nothing and name no foreign revision, so the narrow git-guard rules did not
either.

Why it is a `PreToolUse` hook and not a git hook: git ships no `pre-reset`,
`pre-checkout`, `pre-clean` or `pre-stash`. `post-checkout` runs after the
damage, and `reference-transaction` never fires for `git clean` or for a
`reset --hard` that does not move HEAD. `PreToolUse` is the only seam that sees
the command before it runs (ENG-9964).

The command string is parsed rather than regex-matched, because the shapes that
matter are the ones that look indirect:

1. Split into statements on unquoted `;`, newline, `&&`, `||`, `|`, `(`, `)`,
   tokenizing quote-aware so a quoted path with a space stays one token
   (`statements`).
2. Walk the statements in order, tracking `cd`. `cd <primary> && git reset
   --hard` is judged against the cd target, not the payload `cwd`.
3. Skip env assignments and wrapper prefixes (`sudo`, `env`, `command`, ...),
   then require the command word to be `git`.
4. Walk git's global options, composing every `-C` onto the current directory
   and stepping over the value-taking options (`-c`, `--git-dir`,
   `--work-tree`, `--namespace`) so their arguments are not read as the
   subcommand.
5. Classify the subcommand twice, against two closed enumerated sets.
   `discards_worktree`: `reset --hard|--merge|--keep`, `checkout -f` or its
   pathspec form, `switch --force|--discard-changes`, `restore` other than
   `--staged` alone, `clean -f` without `-n`, and any `stash` except `list`,
   `show` and `create`. `mutates_checkout`: `add`, `am`, `apply`, `checkout`,
   `cherry-pick`, `clean -f`, `commit`, `merge`, `mv`, `pull`, `rebase`,
   `reset`, `restore`, `revert`, `rm`, `stash`, `switch`. Both sets are closed
   so an unclassified subcommand fails open rather than becoming a surprise
   refusal.
6. Resolve the target repo. A linked worktree (private git-dir differs from the
   common dir) is the caller's own and always allowed.
7. If the toplevel is protected, read `git status --porcelain`. A dirty tree
   plus a `discards_worktree` hit denies first, naming the entries that would be
   lost and the `git stash create` snapshot that does not touch the tree. A
   foreign revision denies next. Otherwise a `mutates_checkout` hit denies,
   naming the offending path. Every refusal ends at the same `worktree add`
   under `/tmp/worktree/<org>/<repo>/`, derived from the checkout's origin URL.

What stays allowed in a protected checkout: `status`, `log`, `diff`, `show`,
`grep`, `ls-files`, `rev-parse`, `branch`, `fetch`, `worktree list|add`, any
invocation carrying `--dry-run` (plus `-n` for `clean`, `rm` and `mv`, where
that is what it means), and `apply --check|--stat|--numstat|--summary`. The
`stash list|show|create`, `branch <name>` and `worktree add` holes are
deliberate: they are the commands the refusals themselves recommend, so denying
them would leave the advice with no exit.

Cleaning up a protected checkout by hand -- unstaging someone's stray `git add`,
say -- is itself a mutation and is refused. That is what
`CLAUDE_CODE_DISABLE_GIT_GUARD=1` is for, and the refusal names it.

**This guard does not fail open.** Every other hook here returns silently on any
error, on the principle that a broken hook is worse than no hook. That trade is
wrong for the one guard whose failure mode is unrecoverable data loss, so once
`git-guard` knows the target is a protected checkout it denies when it cannot
read `git status`, and says why. Before that point (non-Bash tool, unparseable
payload, no protected list, target not a repo) it still fails open, because
there is nothing it could be protecting.

Behavioral coverage lives in `packages/claude-code/install-check.nix`: a real
primary checkout with uncommitted work plus a linked worktree, asserting the
five destructive shapes deny, the `cd`/`-C`/`sudo`/chained/global-option
evasions deny, the mutating subcommands deny even on a clean checkout, reads
and the recommended rescue commands allow, both deny messages name what the
caller needs, and the kill switch allows silently. The unit tests in
`packages/claude-hooks/src/guards.rs` build a throwaway checkout and a linked
worktree in a tempdir and drive `git_guard_decision` against them directly.

## write-guard (PreToolUse, Bash)

Refuses a shell command whose write targets land in a protected primary
checkout. Same protected list and same worktree prescription as
`worktree-guard`, and the same kill switch --
`CLAUDE_CODE_DISABLE_WORKTREE_GUARD` -- because it is one policy ("never write
in a primary checkout") enforced at a second seam, and an operator who stands it
down means it for both.

It exists because a fence with a gate beside it is not a fence (index#4310). A
subagent whose tool list was `Bash, Read, WebFetch, WebSearch` -- no `Edit`, no
`Write` -- answered a documentation question and then created a module, wrote a
doc, deleted its own module and added a 55-line option to
`packages/agent/home-manager/claude-code.nix`, all inside
`~/.config/nix/ix/index`, all through heredoc redirects, `cp` and `rm`. Nothing
fired: `worktree-guard` judges an edit tool's `file_path` and never sees Bash,
and `git-guard` only classifies `git`. That checkout is a `path:` flake
submodule a workstation evals from, so the dirty tracked files would have
entered the next `hms` switch.

What it sees, per statement of the parsed command (reusing `git-guard`'s
quote-aware tokenizer, its `sh -c` unwrapping and its `cd` tracking):

- **Output redirections**, attached or detached, appending or clobbering: `>`,
  `>>`, `>|`, `2>`, and the `>` that opens a heredoc write (`cat > f <<'EOF'`).
  Input redirections and fd duplications (`2>&1`, `>&2`) write no file and are
  skipped.
- **A closed table of writing commands**, classified by how each one names its
  targets: every operand (`rm`, `mv`, `tee`, `truncate`, `touch`, `mkdir`,
  `mkfifo`, `rmdir`, `unlink`, `shred`), every operand but the mode
  (`chmod`, `chown`, `chgrp`), the last operand only (`cp`, `install`, `ln`,
  `rsync` -- the earlier ones are sources, and reading in a primary checkout is
  always fine), the operands only in the in-place form (`sed -i`, `perl -pi`),
  and the working directory alone where the targets arrive on stdin (`patch`,
  `xargs <writer>`). Like `mutates_checkout`, the table is closed: a command
  nobody has classified allows rather than refuses.
- **A directory target judged from inside itself**, so `rm -rf <primary>` is
  refused rather than measured against whatever repo the parent sits in.

`git` is deliberately absent from the table. `git-guard` already refuses `git
apply`, `git checkout` and every other mutating subcommand in a protected
checkout, and two guards firing on one command would print two refusals with two
prescriptions.

**What it cannot see**, and no shell-free parse ever will:

- a path whose first component is an expansion (`> "$OUT/x"`,
  `> $(dirname "$f")/x`) or a glob. A path whose *literal prefix* has a `/`
  still resolves (`> doc/$name.md` is refused);
- a write performed by an interpreter rather than the shell: `python -c`,
  `node -e`, `awk '{print > "f"}'`, a redirection inside a quoted program, or a
  script file the command merely names;
- `find -delete`/`-exec`, `tar -x`, `unzip`, `dd of=`, `$EDITOR`, and build
  tools that write where their own config says (`make`, `npm`,
  `nix build --out-link`);
- anything reached over ssh or through a container.

Those remain the parent prompt's job, and the guard is a net for accidents
rather than a sandbox. It also fails open on its edges (kill switch, non-Bash
tool, no protected list, unparseable payload, unresolvable target), on the same
principle as every other hook here.

Behavioral coverage lives in `packages/claude-code/install-check.nix` next to
`git-guard`'s, reusing the same real primary checkout and linked worktree: every
write shape above denies in the primary checkout, the
absolute/`cd`/`sh -c`/buried-in-a-chain evasions deny, reads and writes aimed
outside the checkout allow, the same commands in the linked worktree allow, the
refusal names the offending token and the worktree recipe, and the kill switch
allows silently. The unit tests in `packages/claude-hooks/src/guards.rs` drive
`write_guard_decision` against a tempdir checkout and worktree directly.

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
(`packages/claude-code/default.nix:244-285`) with generous per-hook timeouts (5s,
10s, 5s) that sit well past the fail-open budgets. `install-check.nix` asserts
the fail-open behavior, the digest cap, and the guard deny/allow paths.

Unit tests (`src/main.rs:398-483`) cover the char cap, the word and fleet-noun
gates, the protected-glob slash-crossing, the priors score gate and cap, and the
provenance formatting.
