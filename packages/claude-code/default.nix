{
  lib,
  ix,
  stdenv,
  fetchurl,
  runtimeShell,
  makeBinaryWrapper,
  runCommand,
  autoPatchelfHook,
  procps,
  ripgrep,
  git,
  minecraft-sound,
  bubblewrap,
  socat,
  nix,
  gnupg,
  formats,
  jq,
  binName ? "claude",

  # Default posture: bake `--dangerously-skip-permissions` into the wrapper so
  # every session starts with the permission layer skipped. We run a trusted
  # config inside disposable sandboxes (ix guest VMs, the dev image, throwaway
  # checkouts) where a per-tool approval dialog buys nothing and only stalls an
  # agent that has nowhere unsafe to go. Mind the upstream uid-0 guard: the CLI
  # refuses this flag for an unsandboxed root user (no IS_SANDBOX=1 is baked
  # here, since a bare host genuinely is not a sandbox), so root consumers
  # either carry their own IS_SANDBOX=1 wrapper (the dev image does, plus a
  # managed-settings layer) or turn this off with
  # `claude-code.override { dangerouslySkipPermissions = false; }`.
  dangerouslySkipPermissions ? true,

  # Extra settings.json keys to ship through the read-only flagSettings layer
  # (the `--settings` file below), deep-merged UNDER the computed defaults so the
  # keys this package controls always win on a conflict. Lets a consumer keep its whole
  # static Claude config (hooks, statusLine, enabledPlugins, marketplaces, ...)
  # in Nix and out of a hand-maintained ~/.claude/settings.json: flagSettings
  # merges per-key ABOVE user settings and is a separate read-only layer, so it
  # never occupies (or symlinks) the writable settings.json the CLI churns at
  # runtime. `{ }` (default) ships only the computed defaults.
  extraSettings ? { },

  # Shell glob patterns for the durable primary checkouts the PreToolUse
  # worktree guard protects (the claude-hooks `worktree-guard` subcommand): a file-edit tool call
  # whose target resolves into a PRIMARY checkout (git-dir == git-common-dir,
  # i.e. not a linked worktree) whose toplevel matches one of these globs is
  # denied, regardless of the session's cwd. The list deliberately names the
  # long-lived shared checkouts rather than blocking every primary checkout:
  # a scratch clone in /tmp is also "primary" for its own repo and must stay
  # editable. Globs are matched by the shell `case` builtin, where `*` crosses
  # `/`. Override per machine with the colon-separated
  # CLAUDE_CODE_PRIMARY_CHECKOUTS env var; `[ ]` disables the guard.
  primaryCheckouts ? [
    "/home/*/index"
    "/home/*/ix"
  ],

  # Sibling repo packages from the flake package set. lib/packages.nix threads
  # the lazily-recursive set in under this one name so a repo package can
  # depend on another by id without a flat merge into callPackage's top-level
  # namespace (where ids like `btop` or `kitty` would shadow the nixpkgs attrs
  # other packages resolve, and a self-named override like packages/btop would
  # recurse into itself). The overlay eval context does not provide it (the
  # `mcp` package needs `ix.rustWorkspace` rebound to the caller's pkgs, which
  # only the flake package set does), so the overlay build of
  # `pkgs.claude-code` falls back to `{ }` and drops the defaults below that
  # need a sibling.
  repoPackages ? { },

  # MCP servers baked into the wrapper as a generated `--mcp-config=<file>`
  # layer, one plain server per entry (tool prefix `mcp__<name>`). Values use
  # Claude's mcpServers schema (`{ type = "stdio"; command = ...; }` /
  # `{ type = "http"; url = ...; }`). CLI `--mcp-config` layers MERGE: a user's
  # own `--mcp-config` and a discovered project `.mcp.json` still load alongside
  # this set, so baking the flag here replaces the old pattern of consumers
  # symlinkJoin-wrapping this wrapper a second time just to add it. Defaults to
  # the house pair, additions only (no stock tool is disabled or overridden):
  #  - `index`: the ix notebook kernel (`ix-mcp serve`, packages/mcp) over
  #    stdio. Present only when the `mcp` sibling is in scope, i.e. in the
  #    flake package set but not the overlay (see `repoPackages`).
  #  - `exa`: Exa's hosted web-search server over streamable HTTP at
  #    https://mcp.exa.ai/mcp. Keyless works with rate limits; for higher
  #    limits add a keyed copy in user scope (`claude mcp add --transport http
  #    exa "https://mcp.exa.ai/mcp?exaApiKey=..."`), which merges alongside and
  #    is preferred over baking a secret into the world-readable store.
  # `{ }` bakes no flag.
  mcpServers ? {
    exa = {
      type = "http";
      url = "https://mcp.exa.ai/mcp";
    };
  }
  // lib.optionalAttrs (repoPackages ? mcp) {
    index = {
      type = "stdio";
      command = lib.getExe repoPackages.mcp;
      args = [ "serve" ];
    };
  },

  # Text APPENDED to Claude Code's stock system prompt. The string is
  # materialized to a store file and baked into the wrapper as
  # `--append-system-prompt-file=<path>`: passing by path (not inline text)
  # keeps arbitrary content free of shell quoting, and the store path makes the
  # flag one self-contained argv token (see `wrapperFlags` for why every
  # injected option-argument uses the `=` form).
  # Append, never replace: the stock prompt (tool guidance, safety rules,
  # coding conventions) stays intact and these house rules ride on top.
  # Prepended before the user argv so an explicit
  # `--append-system-prompt`/`--append-system-prompt-file` on the CLI still
  # wins (single-value options are last-wins), and a caller who really wants a
  # wholesale replacement can still pass `--system-prompt[-file]`. Defaults to
  # the house prompt below (the shokunin craft ethos plus the pre-v1
  # backward-compatibility engineering rule, plus a preference for working in git
  # worktrees); set to `null` to bake no flag and ship the stock prompt alone.
  # Authored as one paragraph per list element and joined with blank lines, so a
  # rule reads as a self-contained line of source instead of buried in a wall of
  # indented-string prose.
  appendSystemPrompt ? lib.concatStringsSep "\n\n" [
    "Work as a shokunin. Be concise, readable, and clean by default, in code and in prose: it just works."
    "This codebase is pre-v1: no backward compatibility. Design the correct API and migrate every call site in the same change; add aliases, shims, or deprecated paths only when explicitly asked or when a real external consumer is out of reach."
    "One concept, one implementation. When you find duplicated logic or divergent variants, consolidate them into one composable path instead of adding another. A genuinely general helper belongs in the shared library (`lib/`, e.g. `lib/util/`), imported by name, not buried in a single package or copied per call site; promote it to that common home the moment a second consumer appears or the utility is plainly foundational, and keep package-specific glue (CLI flag spellings, schema quirks) in the package."

    "Fix problems at their source. If the cause is upstream, fix it there and open a PR against that project; a local workaround is a last resort and must link the upstream issue or PR."
    "ALWAYS work in a dedicated git worktree on its own branch; never edit the primary checkout. If you are about to change a file there, stop and make a worktree first."
    "Bash cwd resets between calls: address your worktree by absolute path or `git -C <worktree>`, and before any commit or branch operation verify `git rev-parse --show-toplevel` and the current branch match your assigned worktree."

    "When a task splits into genuinely independent pieces, spawn a background subagent per piece, each in its own worktree, committing to `main`; collect results as they finish. Foreground only when you cannot take a single useful step until it returns."

    "Spend the strongest model only on hard, high-stakes work: hand easy tasks to a subagent on a cheaper model. Planning is usually the hard part, so plan on the strongest model and let a cheaper subagent execute the settled plan."
    "Do your work through the index Python kernel (`python_exec` MCP tool), reusing its persistent namespace across turns. Search with the in-process `fff.grep`/`fff.find` (`api()` lists them); never shell out to `rg` or `fd` inside the kernel, where they run non-interactively and silently mislead (`rg` with no path argument searches empty stdin and returns nothing). Repo instructions routing Bash-tool searches through `rg`/`fd` still apply to the Bash tool. Use Bash only when the kernel is wedged: event loop frozen and neither `kernel_trace` nor a fresh `python_exec` revives it."
    "Before any non-trivial task, search fleet history for priors: in the kernel, `import search`, then `await search.semantic(\"<task phrasing>\", source=[\"claude_history\"], top_k=5)`. Route by question type: `shell` for what-is-the-command, `github` for why-is-it-this-way, `claude_history` for how-did-someone-do-this. For broader prior research spawn a cheap-model subagent so raw hits never flood your context. The corpus knows prior decisions, known pitfalls, and whether the thing is already built."
    "Prefer structured primitives over text munging: `view.ls`/`view.tree`/`view.cat` for the filesystem (polars frames, pre-imported), `fff.grep`/`fff.find` for search, and a CLI's JSON mode (`gh --json`, `cargo metadata`, `nix --json`) parsed with `.json()`/`.jsonl()`/`.df()` on the `sh` Output, never awk/sed/string splitting. ONE command per `sh()` call; combine results in Python. Return tabular answers as polars DataFrames."
    "Complete every task fully and autonomously. Never ask for confirmation or say you *will* do something: do it now and report what you did. You are not done until tests pass and your commits are pushed directly to `origin/main`. Pushing to `main` without waiting for CI is the normal case; open a PR and block on checks only when explicitly asked."
    "Admin and force merges are gated on a fresh local build (postmortem ENG-2391, an agent force-landed a red PR): `gh pr merge --admin`, `--force`, or any merge that bypasses required checks is allowed ONLY immediately after a full local build and test run that you ran yourself on the EXACT head SHA being merged, and that passed. Cite the command and its passing output in the message announcing the merge. If the local run fails or you cannot run it, fix the failure or wait for CI; never force-land a red or unverified PR."
    "Never silently change design or scope. When the planned approach stops fitting, stop and surface it, citing what changed; bypassing an abstraction, swapping an API, or relaxing an error to a warning is a decision the user owns, because a reviewer would question it."
    "A denied tool call or guard message is an instruction, not an obstacle. Read it and use the prescribed alternative; never bypass a guard with sed/python rewrites or by disabling the sandbox. If no alternative exists, report the blocker."
    "Squash merges rewrite history: rebasing a stacked branch directly onto `origin/main` replays the parent's already-merged commits and manufactures phantom conflicts. Instead fetch origin, read the parent base with `git cat-file -p refs/branch-metadata/<branch> | jq -r .parentBranchRevision`, then `git rebase --onto origin/main <parentBranchRevision> <branch>`."
    "Once a change merges into `origin/main`, delete its worktree and branch, locally and on the remote."
    "Announce every landing on `origin/main` with a one-line banner: `🚀 Pushed to main: [<summary>](<commit url>)` for a direct push, `🌸 PR merged: [<title or number>](<url>)` for a merged PR. These two emoji are deliberate signal and the one exception to the no-decorative-emoji rule. Also play `minecraft-sound play block/amethyst/resonate1`."
    "File an issue the moment you hit something worth capturing: a flaw in your own approach a later run should avoid, index friction (misleading tool surface, context-flooding output, a wedged kernel, a correction, a plainly better implementation), or anything that slowed you down. GitHub issue in the relevant repo (`indexable-inc/index` for index friction), Linear ticket for ix work. One observation per report: expected, actual, and the smallest change that would have helped."
    "Use a fenced ```mermaid diagram in issues, PRs, tickets, and design docs when a flow, state machine, architecture, or dependency graph reads better as a picture. Keep it to the one relationship that matters and pair it with a sentence of context."
    "Bug reports to other people must link a runnable minimal reproducible example, not just prose: a self-contained artifact (a `nix-shell` shebang script or small flake) in a GitHub gist. A secret gist is unlisted, not private, so scrub secrets first and use an access-controlled channel when the reproduction is sensitive."
    "Disclose AI authorship in every message another person will read (email, chat, social posts, issues, comments): append an attribution naming your model and version if your context says which model you are, otherwise a generic `(sent by an AI agent via Claude Code)`. Does not apply to replies to the user you are working with."
    "Never use em dashes, anywhere: restructure the sentence, or use a colon, comma, parentheses, or two sentences."
    "Other developers are actively working in this codebase. Treat unmerged branches as unfinished for a reason you may not see, and never work on someone else's feature or branch without coordinating."
  ],

  # Only the flake package set injects the Nushell writer; the overlay eval
  # context does not. The updater is a maintainer-facing flake output, so the
  # overlay build of `pkgs.claude-code` simply omits `passthru.updateScript`.
  writeNushellApplication ? null,
}:

let
  # Version and per-platform SRI hashes are generated, never hand-edited. Bump
  # with `nix run .#claude-code.updateScript -- <version>`, which refetches
  # Anthropic's per-version manifest and rewrites manifest.json. We pin by raw
  # version (not the npm `latest` tag) because Anthropic ships new builds to the
  # `next` prerelease tag days before promoting them to `latest`, and every
  # channel that normally surfaces an upgrade (the built-in updater, `claude
  # doctor`, sadjow/claude-code-nix) only watches `latest`.
  manifest = lib.importJSON ./manifest.json;
  inherit (manifest) version;

  # Env defaults applied through the launcher, declared as data (single source)
  # and rendered into the spec's `env_defaults` below rather than hand-written
  # into the install phase. Set by the launcher only when unset (the old
  # `--set-default`), so an explicit env or settings.json `env` value still
  # overrides per machine. Three groups:
  #  - Output-truncation caps raised to the CLI's built-in maxima: we run a
  #    trusted config (our own CLAUDE.md / AGENTS.md / hooks / MCP servers), so
  #    prefer full output over pruning. BASH_MAX_OUTPUT_LENGTH default 30000
  #    chars (binary clamp 150000); TASK_MAX_OUTPUT_LENGTH default 32000 (clamp
  #    160000); MAX_MCP_OUTPUT_TOKENS default ~25000 tokens (no clamp).
  #  - Feature toggles on by default fleet-wide: agent teams, still gated behind
  #    the EXPERIMENTAL_ env var in this build.
  #  - Context window: default every session to standard 200K Opus 4.8, not the
  #    1M window the `opus` alias is silently auto-upgraded to on
  #    Max/Team/Enterprise/API (1M reads past 200K are uncached and slower per
  #    turn). Per the inline note this is the env knob, not a `model` setting.
  wrapperEnvDefaults = {
    # Drops [1m] variants from /model without touching model selection (a `model`
    # settings key would, since flagSettings outranks user settings.json).
    # Re-enable 1M per machine: `export CLAUDE_CODE_DISABLE_1M_CONTEXT=`.
    CLAUDE_CODE_DISABLE_1M_CONTEXT = 1;
  };
  # Rendered into the launcher spec as `env_defaults` (set only when unset), so
  # an explicit env value still overrides per machine (e.g.
  # `export CLAUDE_CODE_DISABLE_1M_CONTEXT=` re-enables 1M).

  # Settings-key defaults that have no env knob, shipped as a JSON the wrapper
  # injects via `--settings`. The package wraps the binary, so it can carry env
  # vars and CLI flags but not a settings.json *key* directly. `--settings` adds
  # a `flagSettings` layer that merges per-key with the other settings sources
  # (precedence: managed > flagSettings > local > project > user; arrays concat),
  # so it overrides a user's settings.json value but leaves every other key
  # intact, and managed settings can still override it.
  #
  # IMPORTANT: between two `--settings` *flags* the CLI is first-wins (they do
  # NOT merge with each other), so the wrapper injects this file only when the
  # caller passed no `--settings` of their own (the launcher's `conditional_flags`
  # in `launchSpec`): a user's CLI `--settings` applies untouched, and ours
  # applies only when they pass none. Injecting ours unconditionally up front
  # would silently shadow theirs, and the old approach of appending it after the
  # user argv put it inside subcommand argv, where a parser that does not define
  # the option dies (`claude mcp list` -> "error: unknown option '--settings'",
  # issue #1044).
  #   cleanupPeriodDays: keep transcripts + the wrapper's --debug logs ~1yr for
  #     the optimize analysis and troubleshooting (CLI default 30).
  #   skipDangerousModePermissionPrompt (the default): pre-accept the one-time
  #     dangerous-mode warning, which the baked `--dangerously-skip-permissions`
  #     flag alone does not suppress. Honored in every scope except *project* (a
  #     guard against untrusted repos), so it takes effect from this flagSettings
  #     layer. The dev image (images/dev/development-base) enforces the same
  #     posture via managed settings; see its comment for the full rationale.
  #   hooks.UserPromptSubmit (only when the `search` sibling is in scope):
  #     the score-gated ambient-priors hook (claude-hooks `prompt-priors`); see
  #     `claudeHooks` below and packages/claude-hooks for the design.
  #   hooks.SessionStart: the context-digest hook (claude-hooks `session-digest`);
  #     see `claudeHooks` below.
  #   hooks.PreToolUse: the worktree isolation guard for file-edit tools
  #     (claude-hooks `worktree-guard`). Shipped from this layer (not a project
  #     .claude/settings.json) on purpose: project hooks only load for
  #     sessions started inside that project, which is exactly the bypass the
  #     guard closes (ENG-2692).
  #   permissions.ask `gh pr merge --admin`/`--force` (ENG-2688, postmortem
  #     ENG-2391): a check-bypassing merge must pause for confirmation so the
  #     local-build gate in the appended system prompt gets applied, not
  #     skimmed. Ask rules (unlike deny) are not enforced under the baked
  #     `--dangerously-skip-permissions`, so under the default posture the
  #     prompt rule is the practical gate and this rule covers consumers who
  #     turn the flag off; deny would be wrong here because an admin merge
  #     after a passing local build on the head SHA is explicitly allowed.
  #   permissions.deny WebSearch/WebFetch (only while the exa MCP server is in
  #     the baked `mcpServers`): one web surface, not two. Exa's
  #     web_search_exa/web_fetch_exa cover both built-ins, so denying the
  #     stock pair stops the model from splitting identical lookups across two
  #     toolsets. Deny rules are enforced in every permission mode, including
  #     the baked `--dangerously-skip-permissions`. Scoped to `mcpServers ?
  #     exa` so a consumer who overrides the server set away gets the
  #     built-ins back instead of no web access at all.

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
  claudeHooks = runCommand "claude-hooks-wrapped" { nativeBuildInputs = [ makeBinaryWrapper ]; } ''
    makeBinaryWrapper ${ix.rustWorkspace.units.binaries."claude-hooks"}/bin/claude-hooks \
      $out/bin/claude-hooks \
      --set IX_GIT ${lib.getExe git} \
      --set IX_DEFAULT_PRIMARY_CHECKOUTS ${lib.escapeShellArg (lib.concatStringsSep ":" primaryCheckouts)} \
      ${lib.optionalString (repoPackages ? search) "--set IX_SEARCH ${lib.getExe repoPackages.search}"}
  '';
  hookCmd = sub: "${claudeHooks}/bin/claude-hooks ${sub}";

  # Caller's extraSettings first, then the computed defaults recursively merged
  # ON TOP, so the keys below always win a conflict while the caller's other
  # keys (hooks, statusLine, ...) pass through.
  settingsDefaults = ix.deepMerge.rhs extraSettings (
    {
      cleanupPeriodDays = 365;
      permissions = {
        ask = [
          "Bash(gh pr merge*--admin*)"
          "Bash(gh pr merge*--force*)"
        ];
      }
      // lib.optionalAttrs (mcpServers ? exa) {
        deny = [
          "WebSearch"
          "WebFetch"
        ];
      };
      hooks = {
        SessionStart = [
          {
            hooks = [
              {
                type = "command";
                command = hookCmd "session-digest";
                # A local file read; generous next to the CLI's 60s default.
                timeout = 5;
              }
            ];
          }
        ];
        PreToolUse = lib.optional (primaryCheckouts != [ ]) {
          matcher = "Edit|MultiEdit|Write|NotebookEdit";
          hooks = [
            {
              type = "command";
              command = hookCmd "worktree-guard";
              # The hook runs a handful of local `git rev-parse` calls; well
              # past that something is wedged and failing open beats stalling
              # every edit.
              timeout = 10;
            }
          ];
        };
      }
      // lib.optionalAttrs (repoPackages ? search) {
        UserPromptSubmit = [
          {
            hooks = [
              {
                type = "command";
                command = hookCmd "prompt-priors";
                # Generous ceiling over the script's own 2s search budget; the
                # CLI default is 60s, far past fail-open.
                timeout = 5;
              }
            ];
          }
        ];
      };
    }
    // lib.optionalAttrs dangerouslySkipPermissions {
      skipDangerousModePermissionPrompt = true;
    }
  );
  settingsDefaultsFile =
    (formats.json { }).generate "claude-code-default-settings.json"
      settingsDefaults;

  mcpConfigFile = (formats.json { }).generate "claude-code-mcp-config.json" {
    inherit mcpServers;
  };

  # Dirs prepended to PATH at launch (the old `--prefix PATH :`): ps for process
  # checks, the pinned ripgrep, the house minecraft-sound chime, and the Linux
  # sandbox helpers. Passed to the launcher as `path_prepend` (it joins them
  # ahead of the caller's PATH).
  pathPrepend = map (p: "${lib.getBin p}/bin") (
    [
      procps
      ripgrep
      minecraft-sound
    ]
    ++ lib.optionals stdenv.hostPlatform.isLinux [
      bubblewrap
      socat
    ]
  );

  # Every flag the wrapper injects, PREPENDED before the user argv. Two hard
  # rules, both learned from real breakage:
  #
  #  - Prepend, never append. Root-level options parse before subcommand
  #    dispatch, so `claude --settings=F mcp list` works; an appended flag lands
  #    inside the subcommand's argv, where a parser that does not define the
  #    option dies ("error: unknown option '--settings'", issue #1044).
  #  - An option-argument always rides in the `=` form, one self-contained argv
  #    token. The space form is a landmine: `--mcp-config <configs...>` is
  #    variadic and swallows the next positional (`claude agents` -> "MCP config
  #    file not found: ./agents"), and an optional-value flag does the same.
  #
  # `--debug` is such an optional-value flag (`--debug [filter]`: `--debug
  # agents` parses "agents" as the filter and then rejects the rest), and it has
  # no value spelling for "everything", so it cannot take the `=` form. It is
  # safe ONLY because the unconditional `--thinking-display=...` follows it;
  # never let an optional-value flag sit last in this list.
  #
  # Why each flag:
  #  - `--debug`: write operational telemetry (HTTP/API timings, auto-mode
  #    classifier, MCP/LSP lifecycle, startup phases, permission decisions) to
  #    ~/.claude/debug/ for troubleshooting and the optimize history analysis.
  #    It does not pollute `claude -p` stdout (verified). Those logs prune on
  #    the cleanupPeriodDays sweep, so settingsDefaults ships a long retention.
  #  - `--thinking-display=summarized`: the API default DIFFERS BY MODEL:
  #    `thinking.display` defaulted to "summarized" through Opus/Sonnet 4.6 but
  #    Anthropic silently flipped it to "omitted" on Opus 4.7/4.8 (faster
  #    time-to-first-token, thinking blocks arrive with an empty `thinking`
  #    field), so on the latest Opus the live UI shows nothing and the
  #    transcript persists no reasoning. `showThinkingSummaries` does NOT fix it
  #    (renderer-only; anthropics/claude-code#49268 root cause, #63358 for Opus
  #    4.8); this hidden flag is the only lever that works (verified on 2.1.159).
  #    Safe for Haiku (already summarized by default), and unlike
  #    CLAUDE_CODE_EXTRA_BODY it does not force `type:adaptive`, which Haiku
  #    rejects. We trade the TTFT win for visible reasoning fleet-wide; an
  #    explicit later `--thinking-display=omitted` on the CLI still wins
  #    (single-value options are last-wins).
  #  - `--dangerously-skip-permissions` (see its arg comment).
  wrapperFlags = [
    "--debug"
    "--thinking-display=summarized"
  ]
  ++ lib.optional dangerouslySkipPermissions "--dangerously-skip-permissions"
  ++
    lib.optional (appendSystemPrompt != null)
      "--append-system-prompt-file=${builtins.toFile "claude-code-append-system-prompt.txt" appendSystemPrompt}"
  ++ lib.optional (mcpServers != { }) "--mcp-config=${mcpConfigFile}";

  envEntries = attrs: lib.mapAttrsToList (key: value: { inherit key value; }) attrs;

  # The launch spec consumed by the shared Rust launcher (packages/config-launch):
  # it sets env/PATH, prepends `wrapperFlags`, injects `--settings` only when the
  # caller passed none (the CLI is first-wins between two `--settings` flags),
  # then execs the real binary preserving argv0. The store output is read-only so
  # the bundled self-updater could never write: DISABLE_AUTOUPDATER turns it off,
  # the install checks are skipped, and USE_BUILTIN_RIPGREP=0 pins search to the
  # Nix ripgrep on PATH so the wrapper owns the version pin. `target` is an
  # `@helper@` placeholder substituted at install time (the real binary lives
  # under `$out/libexec`, unknowable here). Covered by the installCheck argv
  # tests below.
  launchSpec = (formats.json { }).generate "claude-code-launch-spec.json" {
    target = "@helper@";
    env = envEntries {
      DISABLE_AUTOUPDATER = "1";
      DISABLE_INSTALLATION_CHECKS = "1";
      USE_BUILTIN_RIPGREP = "0";
    };
    env_defaults = envEntries (lib.mapAttrs (_: toString) wrapperEnvDefaults);
    path_prepend = pathPrepend;
    flags = wrapperFlags;
    conditional_flags = [
      {
        unless_present = [ "--settings" ];
        flags = [ "--settings=${settingsDefaultsFile}" ];
      }
    ];
  };

  inherit (stdenv.hostPlatform) system;
  target =
    manifest.platforms.${system}
      or (throw "claude-code: no prebuilt binary for ${system}; supported: ${lib.concatStringsSep ", " (builtins.attrNames manifest.platforms)}");

  # Primary host is the Anthropic-branded CDN so the source is verifiable; the
  # GCS bucket is the direct origin and stays as a mirror if the CDN is down.
  # The hash pin guarantees both resolve to identical bytes, so this is a
  # mirror list, not a behavioral fallback.
  nativeBinary = fetchurl {
    urls = [
      "https://downloads.claude.ai/claude-code-releases/${version}/${target.slug}/claude"
      "https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases/${version}/${target.slug}/claude"
    ];
    inherit (target) hash;
  };

  # Refreshes manifest.json from Anthropic's published per-version manifest,
  # converting its hex checksums to the SRI hashes the fetcher pins. The slug
  # map lives here as the single owner; default.nix only reads it back. The
  # updater fails closed unless the manifest's detached GPG signature verifies
  # against the pinned release signing key (release-signing-key.asc, fingerprint
  # 31DD DE24 DDFA B679 F42D 7BD2 BAA9 29FF 1A7E CACE, published at
  # downloads.claude.ai/keys/claude-code.asc), so a spoofed manifest cannot
  # inject hashes for attacker-controlled binaries.
  updateScript =
    if writeNushellApplication == null then
      null
    else
      writeNushellApplication {
        name = "claude-code-update";
        runtimeInputs = [
          nix
          gnupg
        ];
        meta.description = "Refresh packages/claude-code/manifest.json to a signed Claude Code release";
        text = ''
          const base = "https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases"
          const signing_key = "${./release-signing-key.asc}"
          const slugs = {
            "aarch64-darwin": "darwin-arm64",
            "x86_64-darwin": "darwin-x64",
            "x86_64-linux": "linux-x64",
            "aarch64-linux": "linux-arm64"
          }

          # Run from the repo root: `nix run .#claude-code.updateScript -- [version]`.
          # Without a version argument it tracks Anthropic's `latest` pointer.
          def main [version?: string] {
            let v = ($version | default (http get $"($base)/latest" | str trim))

            # Download the exact bytes we verify, then parse the same file.
            let work = (mktemp --directory)
            let manifest_path = $"($work)/manifest.json"
            let sig_path = $"($work)/manifest.json.sig"
            http get --raw $"($base)/($v)/manifest.json" | save --force $manifest_path
            http get --raw $"($base)/($v)/manifest.json.sig" | save --force $sig_path

            # Fail closed: only the pinned key lives in this GNUPGHOME, so a
            # zero exit from --verify proves Anthropic signed these exact bytes.
            let gnupghome = (mktemp --directory)
            with-env { GNUPGHOME: $gnupghome } {
              ^gpg --batch --quiet --import $signing_key
              let check = (do { ^gpg --batch --verify $sig_path $manifest_path } | complete)
              if $check.exit_code != 0 {
                error make { msg: $"claude-code: manifest signature verification failed for ($v)\n($check.stderr)" }
              }
            }

            let upstream = (open $manifest_path)
            let platforms = (
              $slugs
              | transpose system slug
              | reduce --fold {} {|row acc|
                  let hex = ($upstream.platforms | get $row.slug | get checksum)
                  let sri = (^nix hash convert --hash-algo sha256 --to sri $hex | str trim)
                  $acc | insert $row.system { slug: $row.slug, hash: $sri }
                }
            )
            let out = "packages/claude-code/manifest.json"
            { version: $v, platforms: $platforms } | to json --indent 2 | save --force $out
            print $"updated ($out) to ($v); signature verified"
          }
        '';
      };
in
stdenv.mkDerivation {
  pname = "claude-code";
  inherit version;

  dontUnpack = true;
  # Stripping rewrites the binary and corrupts the trailer Bun appends to its
  # single-file executables, so the stripped CLI aborts on launch.
  dontStrip = true;
  strictDeps = true;

  nativeBuildInputs = [
    makeBinaryWrapper
  ]
  ++ lib.optional stdenv.hostPlatform.isElf autoPatchelfHook;

  installPhase = ''
    runHook preInstall
    mkdir -p $out/bin $out/libexec $out/share

    # 1Password's "CLI access requested" prompt labels the request with the
    # basename of the process that spawns `op`, which is this real binary rather
    # than the wrapper. Keep it in libexec (off PATH, no leading-dot wrapper
    # convention) and name it for the product so the prompt reads "Claude Code"
    # instead of ".claude-unwrapped". The basename is the human-facing product
    # label, independent of the command alias, since it is only what macOS shows.
    # 1Password docs confirm the prompt shows "the process being authorized (for
    # example, iTerm2 or Terminal)", not the code signature or CFBundleName:
    # https://developer.1password.com/docs/cli/app-integration-security/
    helper="$out/libexec/Claude Code"
    install -m755 ${nativeBinary} "$helper"

    # All flag/env/PATH injection lives in `launchSpec` (see its let-binding and
    # `wrapperFlags` for the per-flag rationale); bake the helper's real path
    # into the @helper@ placeholder, then point the launcher at the spec.
    install -m644 ${launchSpec} $out/share/claude-code-launch-spec.json
    substituteInPlace $out/share/claude-code-launch-spec.json --subst-var-by helper "$helper"
    makeBinaryWrapper ${ix.rustWorkspace.units.binaries."config-launch"}/bin/config-launch \
      $out/bin/${binName} \
      --inherit-argv0 \
      --set IX_LAUNCH_SPEC $out/share/claude-code-launch-spec.json

    runHook postInstall
  '';

  # Argv regression net for the launcher spec, run against a stub target so it is
  # offline and instant. Guards the properties the wrapper exists for: injected
  # flags ride BEFORE the user argv (subcommands keep parsing), every injected
  # option-argument is one `=` token (nothing can swallow a positional), and
  # `--settings` defers to a caller-provided one (the CLI is first-wins between
  # two `--settings` flags). Drives the real generated spec with its `@helper@`
  # target swapped for the stub, through the actual launcher binary (the built
  # `$out/bin/${binName}` forces IX_LAUNCH_SPEC via makeBinaryWrapper `--set`, so
  # the launcher is exercised directly here).
  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck

    launcher=${ix.rustWorkspace.units.binaries."config-launch"}/bin/config-launch
    stub="$PWD/stub"
    printf '%s\n' '#!${runtimeShell}' 'printf "%s\n" "$@"' > "$stub"
    chmod +x "$stub"
    sed "s|@helper@|$stub|" ${launchSpec} > "$PWD/test-spec.json"

    check() {
      local desc="$1" expected="$2"
      shift 2
      local got
      got="$(IX_LAUNCH_SPEC="$PWD/test-spec.json" "$launcher" "$@")"
      if [ "$got" != "$expected" ]; then
        printf 'claude launcher argv check failed: %s\nexpected:\n%s\ngot:\n%s\n' \
          "$desc" "$expected" "$got" >&2
        exit 1
      fi
    }

    check "flags prepend; settings injected when caller passes none" \
      ${
        lib.escapeShellArg (
          lib.concatStringsSep "\n" (
            wrapperFlags
            ++ [
              "--settings=${settingsDefaultsFile}"
              "mcp"
              "list"
            ]
          )
        )
      } \
      mcp list

    check "caller --settings wins; package defaults stay out" \
      ${
        lib.escapeShellArg (
          lib.concatStringsSep "\n" (
            wrapperFlags
            ++ [
              "--settings=/dev/null"
              "-p"
              "hi"
            ]
          )
        )
      } \
      --settings=/dev/null -p hi

    # Session-digest hook net: absent/empty digests stay silent (exit 0, no
    # output, so a host without the ix-context-digest timer loses nothing), a
    # present digest rides additionalContext verbatim, and an oversized digest
    # is hard-capped at 6000 chars.
    mkdir -p digest-home/.cache/ix
    if got="$(HOME="$PWD/no-such-home" ${claudeHooks}/bin/claude-hooks session-digest </dev/null)" && [ -z "$got" ]; then :; else
      printf 'session-digest hook check failed (missing digest): expected silent exit 0, got:\n%s\n' "$got" >&2
      exit 1
    fi
    printf 'Distilled lesson: prefer rg over grep.' > digest-home/.cache/ix/context-digest.md
    got="$(HOME="$PWD/digest-home" ${claudeHooks}/bin/claude-hooks session-digest </dev/null)"
    want='{"hookSpecificOutput":{"hookEventName":"SessionStart","additionalContext":"Distilled lesson: prefer rg over grep."}}'
    if [ "$got" != "$want" ]; then
      printf 'session-digest hook check failed (digest present)\nexpected:\n%s\ngot:\n%s\n' "$want" "$got" >&2
      exit 1
    fi
    printf 'x%.0s' $(seq 9000) > digest-home/.cache/ix/context-digest.md
    cap="$(HOME="$PWD/digest-home" ${claudeHooks}/bin/claude-hooks session-digest </dev/null \
      | ${lib.getExe jq} -r '.hookSpecificOutput.additionalContext | length')"
    if [ "$cap" != 6000 ]; then
      printf 'session-digest hook check failed (cap): expected 6000 chars, got %s\n' "$cap" >&2
      exit 1
    fi
    ${lib.optionalString (repoPackages ? search) ''

      # Fail-open net for the prompt-priors hook: every skip path must exit 0
      # with NO output (anything else would block or pollute the prompt).
      # Offline by construction: each input is rejected by a pre-flight gate
      # (short, no fleet noun, no credential, malformed JSON) before the
      # network-touching search would run. HOME points at an empty dir so the
      # credential gate cannot find a real mgrep token.
      mkdir -p no-home
      hook_silent() {
        local desc="$1" input="$2" got
        if ! got="$(printf '%s' "$input" | HOME="$PWD/no-home" ${claudeHooks}/bin/claude-hooks prompt-priors)" \
          || [ -n "$got" ]; then
          printf 'prompt-priors hook check failed (%s): expected silent exit 0, got:\n%s\n' \
            "$desc" "$got" >&2
          exit 1
        fi
      }
      hook_silent "short prompt skipped" '{"prompt":"fix this typo"}'
      hook_silent "no fleet noun skipped" \
        '{"prompt":"please rename this function to something clearer for readability"}'
      hook_silent "no credential fails open" \
        '{"prompt":"how do we deploy the fleet with colmena to every host"}'
      hook_silent "malformed payload fails open" 'not json'
    ''}

    # Behavioral net for the worktree guard: a real primary checkout plus a
    # linked worktree, built in the sandbox, with the protected-glob env
    # override pointed at the primary. The guard must judge only the TARGET
    # path (allow worktree and out-of-repo edits, deny primary-checkout edits
    # even when the payload cwd lies elsewhere) and honor its kill switch.
    # pwd -P: git resolves physical paths (`--show-toplevel`), so the paths
    # the checks compare and glob against must be physical too.
    checktop="$(pwd -P)"
    primary="$checktop/repos/primary"
    wt="$checktop/repos/wt"
    ${lib.getExe git} init -q "$primary"
    ${lib.getExe git} -C "$primary" -c user.email=ci@ix -c user.name=ci \
      commit -q --allow-empty -m init
    ${lib.getExe git} -C "$primary" worktree add -q "$wt" -b guard-check

    guard() {
      local desc="$1" expect="$2" input="$3" got verdict
      got="$(printf '%s' "$input" \
        | CLAUDE_CODE_PRIMARY_CHECKOUTS="$primary" ${claudeHooks}/bin/claude-hooks worktree-guard)"
      case "$got" in
      ''') verdict=allow ;;
      *'"permissionDecision":"deny"'*) verdict=deny ;;
      *) verdict="unparsed: $got" ;;
      esac
      if [ "$verdict" != "$expect" ]; then
        printf 'worktree guard check failed (%s): expected %s, got %s\n' \
          "$desc" "$expect" "$verdict" >&2
        exit 1
      fi
    }

    guard "edit inside linked worktree" allow \
      "{\"tool_input\":{\"file_path\":\"$wt/a.txt\"}}"
    guard "edit inside primary checkout" deny \
      "{\"tool_input\":{\"file_path\":\"$primary/a.txt\"}}"
    guard "cd evasion: cwd elsewhere, absolute target in primary" deny \
      "{\"cwd\":\"/tmp\",\"tool_input\":{\"file_path\":\"$primary/a.txt\"}}"
    guard "relative target resolves against payload cwd" deny \
      "{\"cwd\":\"$primary\",\"tool_input\":{\"file_path\":\"a.txt\"}}"
    guard "new file under unbuilt primary subdir" deny \
      "{\"tool_input\":{\"file_path\":\"$primary/new/deep/a.txt\"}}"
    guard "new file under unbuilt worktree subdir" allow \
      "{\"tool_input\":{\"file_path\":\"$wt/new/deep/a.txt\"}}"
    guard "edit outside any repo" allow \
      "{\"tool_input\":{\"file_path\":\"$checktop/repos/free.txt\"}}"
    guard "malformed payload fails open" allow 'not json'
    if [ -n "$(printf '%s' "{\"tool_input\":{\"file_path\":\"$primary/a.txt\"}}" \
      | CLAUDE_CODE_DISABLE_WORKTREE_GUARD=1 \
        CLAUDE_CODE_PRIMARY_CHECKOUTS="$primary" ${claudeHooks}/bin/claude-hooks worktree-guard)" ]; then
      printf 'worktree guard check failed: kill switch must allow silently\n' >&2
      exit 1
    fi

    runHook postInstallCheck
  '';

  passthru = lib.optionalAttrs (updateScript != null) {
    inherit updateScript;
  };

  meta = {
    description = "Claude Code, Anthropic's agentic coding tool in the terminal";
    homepage = "https://www.anthropic.com/claude-code";
    # License omitted rather than `licenses.unfree`: the per-system flake
    # package set evaluates nixpkgs without `allowUnfree`, so tagging this
    # vendored binary unfree would block `nix run .#claude-code`. Distribution
    # terms are Anthropic's commercial Claude Code license.
    mainProgram = binName;
    platforms = builtins.attrNames manifest.platforms;
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
}
