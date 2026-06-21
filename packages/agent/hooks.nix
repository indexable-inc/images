# Single source of truth for agent lifecycle hooks, consumed by BOTH agent
# wrappers under ./ (claude-code, codex) — the hook analogue of common.nix
# (which shares systemPrompt + houseServers). Each hook is one subcommand of the
# compiled `claude-hooks` binary (packages/agent/claude-hooks); this module only
# DECLARES which executable + argv runs on which event/matcher for which agent, then
# renders that one list into each agent's native hook-config shape.
#
# Imported from a wrapper's default.nix as
#   (import ../hooks.nix { inherit lib claudeHooks primaryCheckouts; }).claude
# (or `.codex`). `claudeHooks` is the built binary package from
# ./claude-code/hooks.nix; the wrapper already builds it, so it threads it in
# rather than this module rebuilding it.
#
# Claude Code and the Codex fork share the hook event model and the
# `{matcher?, hooks:[{type="command",command,timeout?}]}` shape field-for-field,
# so one declaration list renders to both; the only per-agent difference is which
# declarations apply (the `agents` field) — codex has no `Search` tool and edits
# via apply_patch, so the Search/Edit-matched and review hooks are claude-only.
{
  lib,
  # The built claude-hooks binary (./claude-code/hooks.nix output).
  claudeHooks,
  # Shell globs of primary checkouts the worktree-guard protects; [] disables it.
  primaryCheckouts ? [ ],
  # Personal startup context prints Andrew-specific local notes/inventory. Keep
  # it opt-in so shared wrappers do not leak workstation context into everyone.
  personalStartupContext ? false,
}:
let
  claudeHookSubcommand = sub: {
    package = claudeHooks;
    exeName = "claude-hooks";
    args = [ sub ];
  };

  hookCommands = {
    cachedStartupNotes = claudeHookSubcommand "session-digest";
    hostInventoryBanner = claudeHookSubcommand "session-banner";
    protectedCheckoutGuard = claudeHookSubcommand "worktree-guard";
    nixCargoGuard = claudeHookSubcommand "cargo-guard";
    shellHabitGuard = claudeHookSubcommand "bash-habits-guard";
    indexedSearchGuard = claudeHookSubcommand "search-guard";
    subagentCacheLookup = claudeHookSubcommand "subagent-cache-lookup";
    reviewEditLogger = claudeHookSubcommand "review-log-edit";
    stopReviewGate = claudeHookSubcommand "review-gate";
    frictionIssueReporter = claudeHookSubcommand "friction-report";
    subagentCachePopulate = claudeHookSubcommand "subagent-cache-populate";
  };

  renderCommand =
    command:
    lib.escapeShellArgs (
      [
        (lib.getExe' command.package command.exeName)
      ]
      ++ command.args
    );

  # One list per hook event. Hook fields:
  #   command   executable package, binary name, and argv
  #   matcher   optional tool-name matcher (omit for always-run events)
  #   timeout   optional per-hook timeout (s); omit for the CLI default
  #   agents    which agents get it; defaults to both
  #   enable    optional bool gate (drops the declaration when false)
  declarations = {
    # SessionStart context: cached startup notes plus the live host banner and
    # ~/Projects checkout inventory. Both only print context; harmless on codex.
    SessionStart = [
      # WHAT: Print ~/.cache/ix/context-digest.md when that cache file exists.
      # WHY: Carry durable local lessons into a fresh agent session without
      # making every wrapper rebuild or recompute that summary.
      # ADDED: Andrew Gazelka, 2026-06-20, #1476.
      {
        command = hookCommands.cachedStartupNotes;
        enable = personalStartupContext;
        timeout = 5;
      }
      # WHAT: Print the current hostname and local ~/Projects checkout list.
      # WHY: Give the agent enough machine/repo context to avoid guessing where
      # nearby worktrees live.
      # ADDED: Andrew Gazelka, 2026-06-20, #1476.
      {
        command = hookCommands.hostInventoryBanner;
        enable = personalStartupContext;
        timeout = 5;
      }
    ];

    PreToolUse = [
      # Deny edits whose target resolves into a protected primary checkout. The
      # Edit/Write matcher is claude-shaped (codex edits via apply_patch).
      # WHAT: Reject file-write tool calls whose target is a primary checkout.
      # WHY: Keep agent edits in the intended worktree instead of accidentally
      # modifying the user's protected source checkout.
      # ADDED: Andrew Gazelka, 2026-06-20, #1476.
      {
        matcher = "Edit|MultiEdit|Write|NotebookEdit";
        command = hookCommands.protectedCheckoutGuard;
        timeout = 10;
        agents = [ "claude" ];
        enable = primaryCheckouts != [ ];
      }

      # Bash guards: steer cargo to nix in the monorepos, and catch shell
      # anti-patterns. Codex's shell tool is matcher-aliased to "Bash", so both.
      # WHAT: Inspect shell commands for direct cargo use in repos that expect Nix.
      # WHY: Avoid bypassing the pinned toolchain and cache setup encoded in the
      # repo's Nix entry points.
      # ADDED: Andrew Gazelka, 2026-06-20, #1476.
      {
        matcher = "Bash";
        command = hookCommands.nixCargoGuard;
      }
      # WHAT: Inspect shell commands for patterns that often break agent runs.
      # WHY: Catch known footguns at the boundary where the model still has a
      # chance to choose a safer command.
      # ADDED: Andrew Gazelka, 2026-06-20, #1476.
      {
        matcher = "Bash";
        command = hookCommands.shellHabitGuard;
      }

      # Deny the built-in Search tool (claude-only: codex has no Search tool).
      # WHAT: Block Claude's built-in Search tool.
      # WHY: Keep code search on the indexed MCP/search tools, where results are
      # repo-aware and consistent with the rest of this wrapper.
      # ADDED: Andrew Gazelka, 2026-06-20, #1476.
      {
        matcher = "^Search$";
        command = hookCommands.indexedSearchGuard;
        agents = [ "claude" ];
      }

      # Subagent investigation cache (ENG-4665): serve a fresh prior read-only
      # investigation instead of re-running it cold. Always on; the hook fails
      # open to a cold run when SUBAGENT_CACHE_URL is unset or the daemon is
      # unreachable, so it is inert off the tailnet.
      # WHAT: Look up a recent matching read-only subagent investigation.
      # WHY: Reuse expensive exploration when the same question comes back, while
      # failing open to a normal subagent if the cache is unavailable.
      # ADDED: hari, 2026-06-20, #1475 / ENG-4665.
      {
        matcher = "Agent";
        command = hookCommands.subagentCacheLookup;
        timeout = 15;
        agents = [ "claude" ];
      }
    ];

    # Review gate edit logger. Claude-only — codex edits via apply_patch, which
    # this matcher never sees, so the gate could never arm there.
    PostToolUse = [
      # WHAT: Record that the session changed files through Claude edit tools.
      # WHY: Arm the Stop hook only after edits, so review is required for changed
      # work but not for read-only sessions.
      # ADDED: Andrew Gazelka, 2026-06-20, #1476.
      {
        matcher = "Write|Edit|MultiEdit|NotebookEdit";
        command = hookCommands.reviewEditLogger;
        agents = [ "claude" ];
      }
    ];

    Stop = [
      # Review gate stop check, paired with review-log-edit above.
      # WHAT: Block session stop until the changed work has had one review pass.
      # WHY: Prevent edited code from being handed back without the local review
      # gate running at least once.
      # ADDED: Andrew Gazelka, 2026-06-20, #1476.
      {
        command = hookCommands.stopReviewGate;
        agents = [ "claude" ];
      }

      # Friction mining on every stop: analyze the transcript delta in the
      # background and file genuine friction to Linear. Reads both transcript
      # dialects, so both agents. Self-gates on the git author being an ix
      # contributor (compiled-in), replacing the old conditions/ix-contributor
      # wrapper.
      # WHAT: Analyze the new transcript tail for repeated agent friction.
      # WHY: File real workflow issues automatically so recurring papercuts become
      # tracked fixes instead of private memory.
      # ADDED: Andrew Gazelka, 2026-06-20, #1476.
      {
        command = hookCommands.frictionIssueReporter;
      }
    ];

    # Capture each finished Claude Code subagent investigation for the cache.
    SubagentStop = [
      # WHAT: Store a completed read-only subagent investigation in the cache.
      # WHY: Make future matching Agent tool calls fast without changing behavior
      # when the cache daemon is missing.
      # ADDED: hari, 2026-06-20, #1475 / ENG-4665.
      {
        command = hookCommands.subagentCachePopulate;
        timeout = 30;
        agents = [ "claude" ];
      }
    ];
  };

  defaults = {
    matcher = null;
    timeout = null;
    agents = [
      "claude"
      "codex"
    ];
    enable = true;
  };

  withDefaults = lib.mapAttrs (_: map (d: defaults // d)) declarations;

  unique = lib.foldl' (acc: x: if lib.elem x acc then acc else acc ++ [ x ]) [ ];

  # Render the declaration attrset into the settings.json/codex `hooks` attrset for
  # one agent: { <Event> = [ { matcher?; hooks = [ { type; command; timeout?; } ]; } ]; }.
  forAgent =
    agent:
    let
      groupsFor =
        hooks:
        let
          mine = builtins.filter (d: d.enable && lib.elem agent d.agents) hooks;
          group =
            matcher:
            {
              hooks = map (
                d:
                {
                  type = "command";
                  command = renderCommand d.command;
                }
                // lib.optionalAttrs (d.timeout != null) { inherit (d) timeout; }
              ) (builtins.filter (d: d.matcher == matcher) mine);
            }
            // lib.optionalAttrs (matcher != null) { inherit matcher; };
        in
        map group (unique (map (d: d.matcher) mine));
    in
    lib.filterAttrs (_: groups: groups != [ ]) (lib.mapAttrs (_: groupsFor) withDefaults);
in
{
  claude = forAgent "claude";
  codex = forAgent "codex";
}
