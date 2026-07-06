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
  bubblewrap,
  socat,
  nix,
  gnupg,
  python3,
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
  extraSettings ? {},
  # Directories baked into the wrapper as `--add-dir=<dir>` flags, one per entry.
  # `--add-dir` grants tool file-access to a directory, AND (the reason this arg
  # exists) Claude Code loads any `<dir>/.claude/skills/` and `<dir>/CLAUDE.md`
  # found under it — the documented exception that makes `.claude/skills` under an
  # added dir discoverable as BARE `/<skill-name>` commands, regardless of the
  # session's cwd. This is the declarative, cwd-independent way to ship a fixed
  # set of skills globally (parallel to how `mcpServers` bakes `--mcp-config`):
  # point an entry at a store dir whose `.claude/skills/<name>/SKILL.md` tree is a
  # materialized `skills.mkSkillsDir` output. The skills the CLI's own
  # `.claude/skills` discovery (project + `~/.claude/skills`) finds still load
  # alongside; this only adds. `[ ]` (default) bakes no flag. See the `=`-form
  # note in `wrapperFlags`: `--add-dir` is variadic, so the space form would
  # swallow the next argv token.
  addDirs ? [],
  # Directories baked into the wrapper as `--plugin-dir=<dir>` flags, one per
  # entry: load a Claude Code plugin (a dir with `.claude-plugin/plugin.json`,
  # bundling its own `skills/`, `agents/`, `hooks/`, `.mcp.json`, ...) for every
  # session. Plugin skills/agents are NAMESPACED (`/<plugin>:<skill>`), unlike the
  # bare names `addDirs` yields, so reach for this when you want a self-contained,
  # provenance-tagged bundle rather than loose global skills. `[ ]` (default)
  # bakes no flag.
  pluginDirs ? [],
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
  # Andrew-only local startup context: cached notes and ~/Projects inventory.
  # Disabled for the shared wrapper because those hooks print workstation-local
  # context that is not meaningful for other users.
  personalStartupContext ? false,
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
  repoPackages ? {},
  # MCP servers baked into the wrapper as a generated `--mcp-config=<file>`
  # layer, one plain server per entry (tool prefix `mcp__<name>`). This is the
  # final Claude `mcpServers` JSON; the default is rendered from the shared
  # `ix.mcp` registry (lib/util/mcp.nix) so `index` is declared once and the
  # Codex wrapper bakes the same server from the same source. CLI `--mcp-config`
  # layers MERGE: a user's own `--mcp-config` and a discovered project
  # `.mcp.json` still load alongside this set, so baking the flag here replaces
  # the old pattern of consumers symlinkJoin-wrapping this wrapper a second time
  # just to add it. Defaults to the default pair, additions only (no stock tool is
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
    (import (ix.paths.packagesRoot + "/agent/common.nix") {inherit lib ix repoPackages;})
      .defaultServers,
  # Claude Code "channels" (research preview, needs claude-code >= 2.1.80): MCP
  # servers whose events push into the running session, so the agent reacts to
  # things that happen while you are away. Our `index` server (baked above via
  # `mcpServers`, packages/mcp) is a channel: kernel `notify(...)` and
  # interactive-resource actions emit `notifications/claude/channel` events. It
  # is our OWN stdio server, not on Anthropic's curated preview allowlist, so it
  # loads with `--dangerously-load-development-channels` rather than `--channels`
  # (the "dangerous" name is because a channel injects text into the session's
  # context — a trust decision; here the source is our own baked server). Each
  # entry is a channel spec: `server:<mcpServersKey>` or
  # `plugin:<name>@<marketplace>`; baked as
  # `--dangerously-load-development-channels <spec>...`. Defaults to the `index`
  # server WHEN it is baked (so notify()/interactive resources reach a session
  # with no per-launch flag), and to nothing otherwise (the overlay build has no
  # `index` server, so referencing it would be a dead flag). A session whose org
  # policy (`channelsEnabled`) disables channels, or that never receives a push,
  # is unaffected. `[ ]` bakes no flag.
  developmentChannels ? lib.optional (mcpServers ? index) "server:index",
  # Rule names dropped from the default house prompt (forwarded to
  # ../system-prompt.nix's `omitRules`). Only affects the computed `systemPrompt`
  # default below; ignored when `systemPrompt` is passed explicitly. Lets a
  # consumer bake a variant minus a rule without restating the whole prompt, e.g.
  # `claude-code.override { omitRules = [ "htmlDeliverable" ]; }`. `[ ]` keeps all.
  omitRules ? [],
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
    (import (ix.paths.packagesRoot + "/agent/common.nix") {
      inherit lib ix repoPackages;
      promptOmitRules = omitRules;
    }).systemPrompt,
  # Writer used to build `passthru.updateScript`. Only the flake package set
  # supplies it (lib/packages.nix); the overlay eval context leaves it null. The
  # updater is a maintainer-facing flake output, so the overlay build of
  # `pkgs.claude-code` simply omits `passthru.updateScript`.
  updateScriptWriter ? null,
}: let
  # Read the package set from `ix`, not a `pkgs` callPackage formal: a `pkgs`
  # arg in the formal set breaks `.override` (astlog no-pkgs-in-callpackage),
  # and the rebound `ix.pkgs` is the set the rest of this file already uses
  # (see `inherit (ix) pkgs` below).
  inherit (ix) pkgs;

  # Version and per-platform SRI hashes are generated, never hand-edited. Bump
  # with `nix run .#claude-code.updateScript -- <version>`, which refetches
  # Anthropic's per-version manifest and rewrites manifest.json. We pin by raw
  # version (not the npm `latest` tag) because Anthropic ships new builds to the
  # `next` prerelease tag days before promoting them to `latest`, and every
  # channel that normally surfaces an upgrade (the built-in updater, `claude
  # doctor`, sadjow/claude-code-nix) only watches `latest`.
  manifest = lib.importJSON ./manifest.json;
  inherit (manifest) version;

  # Prebuilt agent content consumed by the repo SessionStart materializer. Claude
  # Code can load store-backed skills through `--plugin-dir` / `--add-dir`, but
  # bare subagents still have to exist under `.claude/agents`; exporting store
  # paths lets that hook do a plain copy instead of running `nix build` during
  # interactive startup. The overlay package has no sibling `mcp` package in
  # scope, so it skips rendered agents whose MCP frontmatter depends on it.
  agentSkillsDir = ix.skills.mkSkillsDir {inherit pkgs;};
  agentAgentsDir =
    if repoPackages ? mcp
    then let
      definitions = import (ix.paths.packagesRoot + "/agent/subagents.nix") {
        inherit
          ix
          lib
          repoPackages
          ;
      };
    in
      ix.agents.mkAgentsDir {
        inherit pkgs;
        agents = definitions.renderedAgents;
        inherit (definitions) rawFiles;
      }
    else null;

  # Set only when the caller has not already provided an env value.
  wrapperEnvDefaults = {
    # Drops [1m] variants from /model without touching model selection.
    # Re-enable 1M per machine: `export CLAUDE_CODE_DISABLE_1M_CONTEXT=`.
    CLAUDE_CODE_DISABLE_1M_CONTEXT = 1;
  };

  # Settings defaults are injected only when the caller passed no `--settings`;
  # Claude treats repeated settings flags as first-wins.

  # Build the hook runner once; shared policy renders it for each wrapper.
  hookRunner = import (ix.paths.packagesRoot + "/agent/policy/hook-runner.nix") {
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
  # Claude settings.json hook block rendered from shared agent policy.
  sharedHooks = import (ix.paths.packagesRoot + "/agent/policy/hooks.nix") {
    inherit
      lib
      hookRunner
      primaryCheckouts
      personalStartupContext
      ;
  };

  # Claude-native permission deny list rendered from shared agent policy. The
  # gates fold in the native tools each baked MCP server supersedes: with the
  # `index` kernel present the stock shell/file/search tools are denied, and
  # the overlay build (no kernel) keeps them.
  sharedPermissions = import (ix.paths.packagesRoot + "/agent/policy/permissions.nix") {
    inherit lib;
    indexKernelBaked = mcpServers ? index;
    exaSearchBaked = mcpServers ? exa;
  };

  # Caller's extraSettings first, then the computed defaults recursively merged
  # ON TOP, so the keys below always win a conflict while the caller's other
  # keys (hooks, statusLine, ...) pass through.
  settingsDefaults = ix.deepMerge.rhs extraSettings (
    {
      # Keep transcripts and wrapper debug logs long enough for troubleshooting.
      cleanupPeriodDays = 365;
      permissions = {
        # Concatenate manually: deepMerge treats lists as leaves.
        deny = (extraSettings.permissions.deny or []) ++ sharedPermissions.claude.deniedToolPatterns;
      };
      # Full Claude hook set rendered from shared agent policy.
      hooks = sharedHooks.claude;
    }
    // lib.optionalAttrs dangerouslySkipPermissions {
      # Suppress the one-time warning that the skip flag alone still shows.
      skipDangerousModePermissionPrompt = true;
    }
  );
  settingsDefaultsFile =
    (formats.json {}).generate "claude-code-default-settings.json"
    settingsDefaults;

  mcpConfigFile = (formats.json {}).generate "claude-code-mcp-config.json" {
    inherit mcpServers;
  };

  # Dirs prepended to PATH at launch (the old `--prefix PATH :`): ps for process
  # checks, the pinned ripgrep, and the Linux sandbox helpers. Passed to the
  # launcher as `path_prepend` (it joins them ahead of the caller's PATH).
  pathPrepend = map (p: "${lib.getBin p}/bin") (
    [
      procps
      ripgrep
    ]
    ++ lib.optionals stdenv.hostPlatform.isLinux [
      bubblewrap
      socat
    ]
  );

  # Prepend root flags. Use `--opt=value` for every option that takes a value:
  # space-form options can be swallowed by subcommands or variadic flags.
  wrapperFlags =
    [
      # Write ~/.claude/debug telemetry; cleanupPeriodDays controls retention.
      "--debug"
    ]
    # Load our own MCP servers as channels (research preview). This flag is
    # VARIADIC (it consumes every following non-`--` token as a spec), so it must
    # be followed by a `--`-prefixed flag — never placed last, where it would
    # swallow the user's argv (a prompt, a subcommand). It sits here so the always-
    # present `--thinking-display=` below terminates the spec list.
    ++ lib.optionals (developmentChannels != []) (
      ["--dangerously-load-development-channels"] ++ developmentChannels
    )
    ++ [
      # Opus 4.7+ otherwise omits thinking from the UI/transcript.
      "--thinking-display=summarized"
    ]
    # Default posture for sandboxed ix environments.
    ++ lib.optional dangerouslySkipPermissions "--dangerously-skip-permissions"
    # Replace the stock prompt when a house prompt is configured.
    ++ lib.optional (
      systemPrompt != null
    ) "--system-prompt-file=${builtins.toFile "claude-code-system-prompt.txt" systemPrompt}"
    # Bake the shared MCP server set when present.
    ++ lib.optional (mcpServers != {}) "--mcp-config=${mcpConfigFile}"
    # `--add-dir` is variadic, so the `=` form is required.
    ++ map (d: "--add-dir=${d}") addDirs
    # Plugins carry namespaced skills, agents, hooks, and MCP declarations.
    ++ map (d: "--plugin-dir=${d}") pluginDirs;

  envEntries = attrs: lib.mapAttrsToList (key: value: {inherit key value;}) attrs;

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
  launchSpec = (formats.json {}).generate "claude-code-launch-spec.json" {
    target = "@helper@";
    env = envEntries (
      {
        DISABLE_AUTOUPDATER = "1";
        DISABLE_INSTALLATION_CHECKS = "1";
        USE_BUILTIN_RIPGREP = "0";
        IX_CLAUDE_SKILLS_DIR = "${agentSkillsDir}";
      }
      // lib.optionalAttrs (agentAgentsDir != null) {
        IX_CLAUDE_AGENTS_DIR = "${agentAgentsDir}";
      }
    );
    env_defaults = envEntries (lib.mapAttrs (_: toString) wrapperEnvDefaults);
    path_prepend = pathPrepend;
    flags = wrapperFlags;
    conditional_flags = [
      {
        unless_present = ["--settings"];
        flags = ["--settings=${settingsDefaultsFile}"];
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

  stockCli = stdenv.mkDerivation {
    pname = "claude-code-stock";
    inherit version;
    dontUnpack = true;
    dontStrip = true;
    strictDeps = true;
    nativeBuildInputs = lib.optional stdenv.hostPlatform.isElf autoPatchelfHook;
    installPhase = ''
      # shell
      runHook preInstall
      install -D -m755 ${nativeBinary} $out/bin/claude
      runHook postInstall
    '';
  };

  # Maintainer-facing updater that refreshes manifest.json from Anthropic's
  # signed per-version manifest (fails closed on a bad GPG signature); see
  # ./update.nix. Built only when this eval context supplied a writer (the flake
  # package set), so the overlay build of `pkgs.claude-code` omits
  # `passthru.updateScript`.
  updateScript =
    if updateScriptWriter == null
    then null
    else
      import ./update.nix {
        writeNushellApplication = updateScriptWriter;
        inherit nix gnupg;
      };
in
  stdenv.mkDerivation (finalAttrs: {
    pname = "claude-code";
    inherit version;

    # The source is a single fetched binary, not an archive.
    dontUnpack = true;

    # Stripping rewrites the binary and corrupts the trailer Bun appends to its
    # single-file executables, so the stripped CLI aborts on launch.
    dontStrip = true;
    strictDeps = true;

    nativeBuildInputs =
      [
        makeBinaryWrapper
      ]
      ++ lib.optional stdenv.hostPlatform.isElf autoPatchelfHook;

    installPhase = ''
      # shell
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
        hookRunner
        launchSpec
        settingsDefaultsFile
        wrapperFlags
        python3
        binName
        ;
    };

    passthru =
      {
        # Same capture path as extractSystemPrompt, but depends only on the fetched
        # upstream binary so prompt snapshots do not rebuild the wrapped package.
        extractStockSystemPrompt = import ./extract-system-prompt.nix {
          inherit ix;
          inherit (ix) pkgs;
          name = "claude-code-extract-stock-system-prompt";
          stockBinary = "${stockCli}/bin/claude";
        };

        # Prints the stock upstream system prompt (no house overrides) by capturing
        # what the unwrapped libexec helper sends to a local ANTHROPIC_BASE_URL
        # server. See ./extract-system-prompt.nix and ./extract-system-prompt.py.
        extractSystemPrompt = import ./extract-system-prompt.nix {
          inherit ix;
          # Read the package set from `ix` rather than a `pkgs` callPackage formal
          # (which `override` can't reach); same value in both build paths.
          inherit (ix) pkgs;
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
      sourceProvenance = [lib.sourceTypes.binaryNativeCode];
    };
  })
