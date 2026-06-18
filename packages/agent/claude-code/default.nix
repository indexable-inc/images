{
  lib,
  ix,
  pkgs,
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
  # layer, one plain server per entry (tool prefix `mcp__<name>`). This is the
  # final Claude `mcpServers` JSON; the default is rendered from the shared
  # `ix.mcp` registry (lib/util/mcp.nix) so `index` is declared once and the
  # Codex wrapper bakes the same server from the same source. CLI `--mcp-config`
  # layers MERGE: a user's own `--mcp-config` and a discovered project
  # `.mcp.json` still load alongside this set, so baking the flag here replaces
  # the old pattern of consumers symlinkJoin-wrapping this wrapper a second time
  # just to add it. Defaults to the house pair, additions only (no stock tool is
  # disabled or overridden):
  #  - `index`: the ix notebook kernel (`ix-mcp serve`, packages/mcp) over
  #    stdio. Present only when the `mcp` sibling is in scope, i.e. in the
  #    flake package set but not the overlay (see `repoPackages`).
  #  - `exa`: Exa's hosted web-search server over streamable HTTP at
  #    https://mcp.exa.ai/mcp. Keyless works with rate limits; for higher
  #    limits add a keyed copy in user scope (`claude mcp add --transport http
  #    exa "https://mcp.exa.ai/mcp?exaApiKey=..."`), which merges alongside and
  #    is preferred over baking a secret into the world-readable store.
  # `{ }` bakes no flag.
  mcpServers ?
    ix.mcp.toClaudeJson
      (import (ix.paths.packagesRoot + "/agent/common.nix") { inherit lib ix repoPackages; })
      .houseServers,

  # Text used AS Claude Code's system prompt, REPLACING the stock prompt. The
  # string is materialized to a store file and baked into the wrapper as
  # `--system-prompt-file=<path>`: passing by path (not inline text) keeps
  # arbitrary content free of shell quoting, and the store path makes the flag
  # one self-contained argv token (see `wrapperFlags` for why every injected
  # option-argument uses the `=` form).
  # Set, not append: this wholly replaces the stock prompt (tool guidance,
  # safety rules, coding conventions) rather than riding on top of it, so the
  # baked text owns the entire system prompt. Prepended before the user argv so
  # an explicit `--system-prompt`/`--system-prompt-file` on the CLI still wins
  # (single-value options are last-wins), and a caller who wants the stock
  # prompt plus additions can still pass `--append-system-prompt[-file]`.
  # Defaults to the shared house prompt (`systemPrompt` in ../common.nix,
  # authored in ../system-prompt.nix: the shokunin craft ethos plus the pre-v1
  # backward-compatibility engineering rule, plus a preference for working in git
  # worktrees); set to `null` to bake no flag and ship the stock prompt alone.
  systemPrompt ?
    (import (ix.paths.packagesRoot + "/agent/common.nix") { inherit lib ix repoPackages; })
    .systemPrompt,

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
  #   permissions.deny `gh pr merge --admin`/`--force` (ENG-2688, postmortem
  #     ENG-2391: agent force-landed a red PR): admin/force merge is forbidden
  #     outright, not gated. The old `ask` rule was theatre — it only intercepts
  #     the Bash tool, but the kernel `sh()` path (and Bash itself, denied above)
  #     reach `gh pr merge --admin` with no permission layer in front, so the
  #     "pause for confirmation" gate was never real. Deny the Bash patterns
  #     instead (enforced in every mode, including the baked
  #     `--dangerously-skip-permissions`); the uncovered kernel `sh()` path is
  #     bound by the `forceMerge` system-prompt rule's flat prohibition. Always
  #     on, every configuration: a check-bypassing merge is never allowed.
  #   permissions.deny WebSearch/WebFetch (only while the exa MCP server is in
  #     the baked `mcpServers`): one web surface, not two. Exa's
  #     web_search_exa/web_fetch_exa cover both built-ins, so denying the
  #     stock pair stops the model from splitting identical lookups across two
  #     toolsets. Deny rules are enforced in every permission mode, including
  #     the baked `--dangerously-skip-permissions`. Scoped to `mcpServers ?
  #     exa` so a consumer who overrides the server set away gets the
  #     built-ins back instead of no web access at all.
  #   permissions.deny Bash (only while the index kernel MCP server `index` is in
  #     the generated `mcpServers`): the kernel's `python_exec`/`sh()` IS the
  #     shell, so the Bash tool is denied to force every shell call onto the
  #     kernel (one async event loop, live on the dashboard, clean output instead
  #     of the raw-pipe ANSI-mangling path). Gated on the CONFIGURED server, not
  #     `repoPackages ? mcp`: a consumer can keep the `mcp` sibling yet override
  #     `mcpServers = { }` (or drop `index`), and gating on availability would
  #     then deny Bash while shipping no kernel server — a session with no shell
  #     at all. Tying the deny to `mcpServers ? index` means Bash is removed only
  #     when its replacement is actually present. Like the pair above, deny is the
  #     ONE wall the dangerous-mode posture cannot punch through: it holds under
  #     the baked `--dangerously-skip-permissions` (verified two ways — a headless
  #     run with `--disallowedTools Bash` under the skip-flag is refused with "No
  #     such tool available: Bash", and the WebSearch/WebFetch deny above is
  #     already live under the same flag, which is why the model uses exa, not the
  #     built-ins). It is not an `ask`, which the flag WOULD auto-approve.
  #     TRADEOFFS: (1) removes the "Bash only when kernel wedged" fallback; a
  #     wedged kernel is recovered with `kernel_trace` / fresh `python_exec` /
  #     restart, never Bash. (2) the `Bash(gh pr merge*--admin*/--force*)` ask
  #     rules below go unreachable when Bash is denied, so for a consumer who sets
  #     `dangerouslySkipPermissions = false` the permission-level merge gate moves
  #     entirely onto the baked `forceMerge` system-prompt rule (the operative
  #     gate fleet-wide anyway, since the default skip-flag already makes `ask`
  #     inert); the kernel `sh()` path carries no equivalent prompt.
  #   fileSuggestion (only when the `fff-suggest` sibling is in scope): routes
  #     `@`-mention file completion through fff's frecency-ranked fuzzy finder
  #     instead of the CLI's built-in index, via the statusLine-shaped custom
  #     completer hook (per-keystroke command, query on stdin, stdout lines used
  #     in order). Fails open. See its `lib.optionalAttrs` block below for the
  #     full contract and the resident-daemon design.

  # The three hooks (session-digest, worktree-guard, prompt-priors) are
  # subcommands of one compiled binary, wrapped with their tool paths and the
  # baked primary-checkout default; each fails open and silent. See ./hooks.nix
  # for the full design, kill switches, and per-hook rationale.
  claudeHooks = import ./hooks.nix {
    inherit
      lib
      runCommand
      makeBinaryWrapper
      ix
      git
      primaryCheckouts
      repoPackages
      ;
  };
  hookCmd = sub: "${claudeHooks}/bin/claude-hooks ${sub}";

  # Tools denied via the flagSettings `permissions.deny` layer; see the
  # `permissions.deny` bullets in the doc block above for why each, and why deny
  # (unlike ask/allow) holds under the baked `--dangerously-skip-permissions`.
  denyTools =
    lib.optionals (mcpServers ? exa) [
      "WebSearch"
      "WebFetch"
    ]
    ++ lib.optional (mcpServers ? index) "Bash";

  # Caller's extraSettings first, then the computed defaults recursively merged
  # ON TOP, so the keys below always win a conflict while the caller's other
  # keys (hooks, statusLine, ...) pass through.
  settingsDefaults = ix.deepMerge.rhs extraSettings (
    {
      cleanupPeriodDays = 365;
      permissions = {
        # Prepend any caller-supplied deny: `ix.deepMerge.rhs` treats a list as a
        # leaf, so a computed `deny` would REPLACE `extraSettings.permissions.deny`
        # outright and silently drop a consumer's own policy. Concatenate instead
        # so package denies are additive to the caller's.
        deny =
          (extraSettings.permissions.deny or [ ])
          ++ [
            "Bash(gh pr merge*--admin*)"
            "Bash(gh pr merge*--force*)"
          ]
          ++ denyTools;
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
    // lib.optionalAttrs (repoPackages ? fff-suggest) {
      # `@`-mention file completion served by fff (frecency-ranked fuzzy finder)
      # instead of Claude's built-in index. `fileSuggestion` is the CLI's
      # statusLine-shaped custom-completer hook: Claude runs this command per
      # keystroke (5s budget, cwd = project dir), passes `{ query, … }` on stdin,
      # and uses each non-empty stdout line as a suggestion in the returned order
      # (no re-ranking), so fff owns the ranking. The command is the tiny native
      # client half of `fff-suggest`, which round-trips the query to a resident
      # per-project daemon over a unix socket (no Python on the hot path) and
      # fails open: any error exits 0 with no output. Gated on the `fff-suggest`
      # sibling, which (like the `search`/`mcp` siblings) only the flake package
      # set provides, so the overlay build of `pkgs.claude-code` simply omits it.
      fileSuggestion = {
        type = "command";
        command = "${repoPackages.fff-suggest}/bin/fff-suggest query";
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
  ++ lib.optional (
    systemPrompt != null
  ) "--system-prompt-file=${builtins.toFile "claude-code-system-prompt.txt" systemPrompt}"
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

  # Maintainer-facing updater that refreshes manifest.json from Anthropic's
  # signed per-version manifest (fails closed on a bad GPG signature); see
  # ./update.nix. Only the flake package set injects the Nushell writer, so the
  # overlay build of `pkgs.claude-code` omits `passthru.updateScript`.
  updateScript =
    if writeNushellApplication == null then
      null
    else
      import ./update.nix { inherit writeNushellApplication nix gnupg; };
in
stdenv.mkDerivation (finalAttrs: {
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

  # Offline argv + hook regression net driven through the real launcher binary
  # against a stub target; see ./install-check.nix for what each check guards.
  doInstallCheck = true;
  installCheckPhase = import ./install-check.nix {
    inherit
      lib
      runtimeShell
      ix
      git
      jq
      repoPackages
      claudeHooks
      launchSpec
      settingsDefaultsFile
      wrapperFlags
      ;
  };

  passthru = {
    # Prints the stock upstream system prompt (no house overrides) by capturing
    # what the unwrapped libexec helper sends to a local ANTHROPIC_BASE_URL
    # server. See ./extract-system-prompt.nix and ./extract-system-prompt.py.
    extractSystemPrompt = import ./extract-system-prompt.nix {
      inherit ix pkgs;
      stockBinary = "${finalAttrs.finalPackage}/libexec/Claude Code";
      wrappedBinary = "${finalAttrs.finalPackage}/bin/${binName}";
    };
  }
  // lib.optionalAttrs (updateScript != null) {
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
})
