{
  lib,
  ix,
  stdenv,
  fetchurl,
  makeBinaryWrapper,
  autoPatchelfHook,
  procps,
  ripgrep,
  bubblewrap,
  socat,
  nix,
  gnupg,
  formats,
  binName ? "claude",
  # Default posture: start every session in bypass-permissions mode. We run a
  # trusted config inside disposable sandboxes (ix guest VMs, the dev image,
  # throwaway checkouts) where a per-tool approval dialog buys nothing and only
  # stalls an agent that has nowhere unsafe to go. The upstream uid-0 guard still
  # refuses bypass for an unsandboxed root user (no IS_SANDBOX=1 is baked here,
  # since a bare host genuinely is not a sandbox), so it is a no-op exactly where
  # it would be unsafe; sandboxed-root consumers (e.g. the dev image) keep their
  # own IS_SANDBOX=1 wrapper and managed-settings layer. Turn it off with
  # `claude-code.override { dangerouslySkipPermissions = false; }`.
  dangerouslySkipPermissions ? true,
  # Opt-in alternative posture: confine the agent to a fixed allow-list and
  # nothing else. Set to a list of permission rules (typically one MCP server,
  # e.g. `[ "mcp__index" "mcp__index__*" ]`) and the wrapper switches to plain
  # `default` permission mode, `allow`s exactly those rules, and bare-`deny`s
  # every other built-in tool (Bash, Read, Edit, Write, ...), stripping them from
  # the model's context. The agent can then only use the allowed tools; anything
  # else (shell, file IO, HTTP) it must do through whatever those tools expose
  # (e.g. a python/Jupyter MCP kernel). A built-in name listed here is removed
  # from the deny list, so you can also re-allow a specific built-in.
  # `bypassPermissions` cannot express this (it skips the permission layer
  # entirely, so deny rules are silently ignored) and `dontAsk` is intentionally
  # avoided. Takes PRECEDENCE over `dangerouslySkipPermissions`, since bypass
  # would void the deny rules. `null` (default) leaves the normal posture.
  restrictToTools ? null,
  # Extra settings.json keys to ship through the read-only flagSettings layer
  # (the `--settings` file below), deep-merged UNDER the computed defaults so the
  # security-relevant keys this package controls (the restrictToTools / bypass
  # `permissions`) always win on a conflict. Lets a consumer keep its whole
  # static Claude config (hooks, statusLine, enabledPlugins, marketplaces, ...)
  # in Nix and out of a hand-maintained ~/.claude/settings.json: flagSettings
  # merges per-key ABOVE user settings and is a separate read-only layer, so it
  # never occupies (or symlinks) the writable settings.json the CLI churns at
  # runtime. `{ }` (default) ships only the computed defaults.
  extraSettings ? { },
  # Replace Claude Code's entire system prompt with this text. The string is
  # materialized to a store file and baked into the wrapper as
  # `--system-prompt-file <path>`: passing by path (not inline text) keeps
  # arbitrary content free of shell escaping and, since a store path has no
  # spaces, it survives makeBinaryWrapper's word splitting where an inline
  # `--system-prompt "<text with spaces>"` would shatter into separate argv.
  # This DROPS the default prompt wholesale (tool guidance, safety rules, coding
  # conventions), so the agent only knows what this text says. Baked with
  # `--add-flags` (prepended) so an explicit
  # `--system-prompt`/`--system-prompt-file` on the CLI still wins. Defaults to the
  # house prompt below (the shokunin craft ethos plus the pre-v1
  # backward-compatibility engineering rule, plus a preference for working in git
  # worktrees); set to `null` to bake no flag and keep Claude Code's stock prompt.
  # Authored as one paragraph per list element and joined with blank lines, so a
  # rule reads as a self-contained line of source instead of buried in a wall of
  # indented-string prose.
  systemPrompt ? lib.concatStringsSep "\n\n" [
    "Work as a shokunin (職人): a craftsman devoted to mastering the craft. Be thoughtful. Ship a beautiful, nearly perfect product, and make the code behind it just as beautiful. It just works."
    "This codebase is pre-v1: do not preserve backward compatibility. Design the correct API, then rename and migrate every call site in the same change rather than adding aliases, shims, or deprecated paths. Keep a compatibility layer only when explicitly asked or when a real external consumer is out of reach."
    "Prefer one simple, unified way to do a thing. Favor generic, composable, DRY building blocks over several parallel implementations of the same idea: when you find duplicated logic or divergent variants, consolidate them into one well-designed path instead of adding another. One concept, one implementation."
    "ALWAYS work in a dedicated git worktree, and never edit the primary checkout or do any work outside a worktree. Before you touch a file, create (or switch into) a worktree on its own branch and do everything there, so the main working tree stays clean and independent lines of work stay isolated. If you find yourself about to change files in the primary checkout, stop and make a worktree first."
    "Do your work through the index Python kernel: run code, read files, and shell out with the `python_exec` MCP tool, and reuse its persistent namespace across turns instead of starting over. Search with the kernel's in-process grep and find (`fff.grep` and `fff.find`, which `api()` lists), and never shell out to `rg` or `fd`, which run in a non-interactive subprocess that silently misleads (for instance `rg` with no path argument searches empty stdin and returns nothing). Use the Bash tool only when the Python kernel is completely wedged: its event loop is frozen and neither `kernel_trace` nor a fresh `python_exec` brings it back."
    "Complete every task fully and autonomously. Do not ask for confirmation or pause to check in; carry the work through to a finished state. Never say you *will* do something or merely describe what you are about to do. Perform the action now and report what you actually did."
    "You are not done until your code has been tested and is merged into `origin/main`. Writing the change is not the finish line: run the tests and verify they pass, then land the work on `origin/main` (push or merge your branch) before you report the task complete."
    "Once a change is merged into `origin/main`, always clean up after it: delete the worktree and its branch (locally and on the remote). Leave no stale worktrees or merged branches behind."
    "Whenever you land a change on `origin/main`, announce it to the user with a one-line merge banner so a merge is never ambiguous: write `🌸 PR merged: [<title or number>](<url>)` as a markdown link to the merged pull request (fall back to the merge commit URL when there was no PR). Use 🌸 for a normal merge and 🔥 when the PR was force merged. Lead the line with that emoji. Also mark the merge with a Minecraft sound via `minecraft-sound play <name>`: `block/amethyst/resonate1` (an amethyst chime) for a normal merge, and `random/anvil_land` (an anvil landing) for a force merge."
    "File an issue the moment you hit something worth capturing, rather than only noting it in passing: a flaw in your own reasoning or approach that a later run should avoid, or a problem in the system and tooling. Index friction counts (a confusing or misleading tool surface, output that floods your context, a wedged kernel, a correction that you did the wrong thing, or a task that worked but has a plainly better implementation), and so does anything else that slowed you down or led you astray. File it as a GitHub issue in the relevant repo (`indexable-inc/index` for index friction), or as a Linear ticket when the work is in the ix repo. One concrete observation per report: what you expected, what happened, and the smallest change that would have helped."
    "When you report a bug to other people (a maintainer, a vendor, an issue tracker, a public thread), always include a minimal reproducible example they can run themselves, not just a prose description, and link it from the report. Prefer a runnable, self-contained artifact (for example a `nix-shell` shebang script or a small flake that provisions its own dependencies) so the recipient reproduces in one step, hosted somewhere linkable such as a GitHub gist rather than pasted inline. Treat a secret gist as unlisted, not private: it is shareable by link but not access-controlled, so anyone with the link can read it. Before sharing, scrub the MRE of any secrets, credentials, tokens, or private data, and use a properly access-controlled or confidential channel when the reproduction would otherwise expose sensitive information."
    "When you send a message that another person will read (email, iMessage or other chat, social posts, GitHub issues or comments, or any outbound message on the user's behalf), always disclose that it came from an AI. Append a short attribution naming the model and version, for example `(sent by Claude Opus 4.8)`, so the recipient is never misled about who wrote it. This applies only to messages addressed to other people, not to your replies to the user you are working with."
    "Write in a way that does not require em dashes. Restructure the sentence, or use a colon, a comma, parentheses, or two sentences instead. This applies to everything you produce: prose, code, comments, commit messages, issues, and your replies to the user."
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

  # Env defaults applied through the wrapper, declared as data (single source)
  # and derived into flags below rather than hand-written into the install phase.
  # `--set-default` (not `--set`) so an explicit env or settings.json `env` value
  # still overrides per machine. Three groups:
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
    BASH_MAX_OUTPUT_LENGTH = 150000;
    TASK_MAX_OUTPUT_LENGTH = 160000;
    MAX_MCP_OUTPUT_TOKENS = 200000;
    CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS = 1;
    # Drops [1m] variants from /model without touching model selection (a `model`
    # settings key would, since flagSettings outranks user settings.json).
    # Re-enable 1M per machine: `export CLAUDE_CODE_DISABLE_1M_CONTEXT=`.
    CLAUDE_CODE_DISABLE_1M_CONTEXT = 1;
    # Self-driving autonomy off fleet-wide. Background tasks are a
    # `run_in_background` *parameter* (no tool to disallow), so this env knob is
    # the only lever; the cron family is also dropped via `--disallowedTools`
    # below, with this as defense in depth. `--set-default`, so a machine opts
    # back in with `export CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=` /
    # `export CLAUDE_CODE_DISABLE_CRON=`.
    CLAUDE_CODE_DISABLE_BACKGROUND_TASKS = 1;
    CLAUDE_CODE_DISABLE_CRON = 1;
  };
  envDefaultFlags = lib.concatLists (
    lib.mapAttrsToList (name: value: [
      "--set-default"
      name
      (toString value)
    ]) wrapperEnvDefaults
  );

  # Autonomy tools removed from every session: self-watching (Monitor),
  # self-scheduling (ScheduleWakeup, the cron family, and RemoteTrigger, which
  # creates/runs claude.ai routines server-side), and the user-interrupting
  # PushNotification. `--disallowedTools` drops them from the model's tool set
  # regardless of permission mode; `permissions.deny` would not, since the
  # default `bypassPermissions` posture skips the permission layer. Monitor,
  # PushNotification, and RemoteTrigger are server-gated with no env knob, so
  # this flag is their only off-switch.
  disallowedAutonomyTools = [
    "Monitor"
    "ScheduleWakeup"
    "RemoteTrigger"
    "PushNotification"
    "CronCreate"
    "CronDelete"
    "CronList"
  ];

  # Lean code-execution agent: the only surface this build needs is the index MCP
  # (its `mcp__index__*` tools, e.g. python_exec, which MCP namespacing leaves
  # untouched by `--disallowedTools`). Drop the built-in meta-tools so the model
  # works turn-by-turn through code execution instead of branching into
  # structured task lists, agent teams, worktrees, or multiple-choice prompts.
  # Plan mode (EnterPlanMode/ExitPlanMode) is dropped too: this agent plans and
  # executes turn-by-turn through code execution rather than branching into the
  # built-in plan/approve flow.
  # Removed regardless of permission mode, same as the autonomy list.
  disallowedMetaTools = [
    "AskUserQuestion"
    "EnterPlanMode"
    "ExitPlanMode"
    "TaskCreate"
    "TaskList"
    "TaskGet"
    "TaskUpdate"
    "TeamCreate"
    "TeamDelete"
    "SendMessage"
    "EnterWorktree"
    "ExitWorktree"
    "WaitForMcpServers"
  ];

  # Settings-key defaults that have no env knob, shipped as a JSON the wrapper
  # injects via `--settings`. The package wraps the binary, so it can carry env
  # vars and CLI flags but not a settings.json *key* directly. `--settings` adds
  # a `flagSettings` layer that merges per-key with the other settings sources
  # (precedence: managed > flagSettings > local > project > user; arrays concat),
  # so it overrides a user's settings.json value but leaves every other key
  # intact, and managed settings can still override it.
  #
  # IMPORTANT: between two `--settings` *flags* the CLI is first-wins (they do
  # NOT merge with each other), so this is injected with `--append-flags` (last
  # in argv): a user who passes their own `--settings` on the CLI wins (theirs
  # comes first), and ours applies only when they pass none. `--add-flags` would
  # prepend ours and silently shadow a user's `--settings`.
  #   cleanupPeriodDays: keep transcripts + the wrapper's --debug logs ~1yr for
  #     the optimize analysis and troubleshooting (CLI default 30).
  #   permissions.{allow,deny} (only when `restrictToTools` is set, opt-in):
  #     confine the agent to that allow-list and strip every other built-in tool
  #     from context. Arrays concat across layers and a deny at any scope wins, so
  #     a downstream user/project/local settings file cannot un-deny these; the
  #     only un-lock is dropping the nix `restrictToTools` override. Caveat: a
  #     managed `bypassPermissions` skips the whole permission layer, so these
  #     deny rules would be inert under one.
  #   permissions.defaultMode + skipDangerousModePermissionPrompt (the default,
  #     unless `restrictToTools` takes precedence): start in bypass mode and
  #     pre-accept the one-time dangerous-mode warning. Both keys are
  #     required: managed/flag bypass alone does not suppress that warning.
  #     skipDangerousModePermissionPrompt is honored in every scope except
  #     *project* (a guard against untrusted repos), so it takes effect from this
  #     flagSettings layer. Same two keys the dev image
  #     (images/dev/development-base) enforces via managed settings; see its
  #     comment for the full rationale.
  #
  # Bare tool names as deny rules remove the built-in tool from the model's
  # context entirely (per the permissions docs), not merely gate it behind a
  # prompt, so the agent never sees a shell/file/web/subagent tool. Read-only Bash
  # (`cat`, `ls`, ...) is otherwise always allowed in every mode, so denying bare
  # `Bash` is what actually closes the shell.
  deniedBuiltinTools = [
    "Agent"
    "Bash"
    "BashOutput"
    "Edit"
    "Glob"
    "Grep"
    "KillShell"
    "ListMcpResources"
    "NotebookEdit"
    "PowerShell"
    "Read"
    "ReadMcpResource"
    "Skill"
    "SlashCommand"
    "Task"
    "TodoWrite"
    "WebFetch"
    "WebSearch"
    "Write"
  ];

  # The lockdown is active whenever the caller pins an allow-list.
  restrictTools = restrictToTools != null;

  # Caller's extraSettings first, then the computed defaults recursively merged
  # ON TOP, so the security-relevant `permissions`/bypass keys below always win a
  # conflict while the caller's other keys (hooks, statusLine, ...) pass through.
  settingsDefaults = ix.deepMerge.rhs extraSettings (
    {
      cleanupPeriodDays = 365;
    }
    // lib.optionalAttrs restrictTools {
      permissions = {
        allow = restrictToTools;
        # A built-in named in the allow-list is re-allowed by dropping it here.
        deny = lib.subtractLists restrictToTools deniedBuiltinTools;
      };
    }
    # restrictToTools takes precedence: bypass would skip the permission layer and
    # void its deny rules, so the two never co-set `permissions` (a shallow //
    # merge would otherwise clobber allow/deny with defaultMode).
    // lib.optionalAttrs (dangerouslySkipPermissions && !restrictTools) {
      permissions.defaultMode = "bypassPermissions";
      skipDangerousModePermissionPrompt = true;
    }
  );
  settingsDefaultsFile =
    (formats.json { }).generate "claude-code-default-settings.json"
      settingsDefaults;

  # System-prompt override (see the `systemPrompt` arg). Materialize the text to
  # a store file and add `--system-prompt-file <path>` as makeBinaryWrapper args.
  # escapeShellArgs emits the `--system-prompt-file <path>` pair as one shell
  # word so makeBinaryWrapper re-splits it into the two argv tokens the CLI wants;
  # when unset the list is empty and contributes nothing.
  systemPromptWrapperArgs = lib.optionals (systemPrompt != null) [
    "--add-flags"
    "--system-prompt-file ${builtins.toFile "claude-code-system-prompt.txt" systemPrompt}"
  ];

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
    mkdir -p $out/bin $out/libexec

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

    # The store output is read-only, so the bundled self-updater can never
    # write; disable it and the install checks, and pin the bundled ripgrep to
    # the Nix one so PATH stays reproducible. The wrapper owns the version pin.
    # Apply our env defaults (see `wrapperEnvDefaults` above).
    #
    # Start in debug mode by default (`--debug`): the CLI writes operational
    # telemetry (HTTP/API timings, auto-mode classifier, MCP/LSP lifecycle,
    # startup phases, permission decisions) to ~/.claude/debug/ for
    # troubleshooting and the optimize history analysis. It does not pollute
    # `claude -p` stdout (verified). Those logs prune on the cleanupPeriodDays
    # sweep, so we also ship a long retention via --settings (see
    # `settingsDefaults` above).
    #
    # Opt back into summarized thinking (`--thinking-display summarized`). The
    # API behavior here DIFFERS BY MODEL: `thinking.display` defaulted to
    # "summarized" on Opus 4.6 / Sonnet 4.6 and earlier, but Anthropic silently
    # flipped it to "omitted" on Opus 4.7 and Opus 4.8 (faster time-to-first-
    # token). With "omitted" the API returns thinking blocks whose `thinking`
    # field is empty (only the encrypted `signature` rides along), so on the
    # latest Opus the live UI shows nothing and the transcript persists no
    # reasoning. The harness never requests "summarized" itself, and
    # `showThinkingSummaries` does NOT fix it (it only drives the ctrl+o
    # renderer + a beta header, wired to nothing that sets the request's
    # display) -- see anthropics/claude-code#49268 (root cause) and #63358
    # (Opus 4.8). The hidden `--thinking-display summarized` flag is the only
    # lever that works, and it is verified to restore readable Opus-4.8 thinking
    # on 2.1.159. We want the reasoning for steering and for the optimize
    # analysis, so we trade the TTFT win for visible thinking fleet-wide. Safe
    # for Haiku (it already defaults to "summarized"); unlike CLAUDE_CODE_EXTRA_
    # BODY this does not force `type:adaptive`, which Haiku rejects. Via
    # `--add-flags` (prepended) so an explicit `--thinking-display omitted` on
    # the CLI still wins for anyone who wants the latency back.
    makeBinaryWrapper "$helper" $out/bin/${binName} \
      --inherit-argv0 \
      --add-flags --debug \
      --add-flags "--thinking-display summarized" \
      --add-flags "--disallowedTools ${
        lib.concatStringsSep "," (disallowedAutonomyTools ++ disallowedMetaTools)
      }" \
      --append-flags "--settings ${settingsDefaultsFile}" \
      ${lib.escapeShellArgs systemPromptWrapperArgs} \
      --set DISABLE_AUTOUPDATER 1 \
      --set DISABLE_INSTALLATION_CHECKS 1 \
      --set USE_BUILTIN_RIPGREP 0 \
      ${lib.escapeShellArgs envDefaultFlags} \
      --prefix PATH : ${
        lib.makeBinPath (
          [
            procps
            ripgrep
          ]
          ++ lib.optionals stdenv.hostPlatform.isLinux [
            bubblewrap
            socat
          ]
        )
      }

    runHook postInstall
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
