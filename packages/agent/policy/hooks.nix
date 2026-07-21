# Shared lifecycle hook declarations for Claude Code and Codex wrappers.
{
  lib,
  hookRunner,
  primaryCheckouts ? [],
  personalStartupContext ? false,
}: let
  hookRunnerSubcommand = sub: {
    package = hookRunner;
    exeName = "claude-hooks";
    args = [sub];
  };

  hookCommands = {
    cachedStartupNotes = hookRunnerSubcommand "session-digest";
    hostInventoryBanner = hookRunnerSubcommand "session-banner";
    sessionIdContext = hookRunnerSubcommand "session-id";
    protectedCheckoutGuard = hookRunnerSubcommand "worktree-guard";
    nixCargoGuard = hookRunnerSubcommand "cargo-guard";
    shellHabitGuard = hookRunnerSubcommand "bash-habits-guard";
    indexedSearchGuard = hookRunnerSubcommand "search-guard";
    promptPriors = hookRunnerSubcommand "prompt-priors";
    subagentCacheLookup = hookRunnerSubcommand "subagent-cache-lookup";
    reviewEditLogger = hookRunnerSubcommand "review-log-edit";
    stopReviewGate = hookRunnerSubcommand "review-gate";
    stopRetroGate = hookRunnerSubcommand "retro-gate";
    wakeupArmLogger = hookRunnerSubcommand "wakeup-log";
    stopWakeupGate = hookRunnerSubcommand "wakeup-gate";
    frictionIssueReporter = hookRunnerSubcommand "friction-report";
    subagentCachePopulate = hookRunnerSubcommand "subagent-cache-populate";
  };

  renderCommand = command:
    lib.escapeShellArgs (
      [
        (lib.getExe' command.package command.exeName)
      ]
      ++ command.args
    );

  declarations = {
    SessionStart = [
      # Unconditional: agents key session-scoped artifacts (status boards,
      # scratch dirs) by session id, and nothing else tells them their own id.
      {
        command = hookCommands.sessionIdContext;
        timeout = 5;
      }
      {
        command = hookCommands.cachedStartupNotes;
        enable = personalStartupContext;
        timeout = 5;
      }
      {
        command = hookCommands.hostInventoryBanner;
        enable = personalStartupContext;
        timeout = 5;
      }
    ];

    PreToolUse = [
      # Claude edit tools carry file paths; Codex edits through apply_patch.
      {
        matcher = "Edit|MultiEdit|Write|NotebookEdit";
        command = hookCommands.protectedCheckoutGuard;
        timeout = 10;
        agents = ["claude"];
        enable = primaryCheckouts != [];
      }
      {
        matcher = "Bash";
        command = hookCommands.nixCargoGuard;
      }
      {
        matcher = "Bash";
        command = hookCommands.shellHabitGuard;
      }
      {
        matcher = "^Search$";
        command = hookCommands.indexedSearchGuard;
        agents = ["claude"];
      }
      {
        matcher = "Agent";
        command = hookCommands.subagentCacheLookup;
        timeout = 15;
        agents = ["claude"];
      }
    ];

    UserPromptSubmit = [
      {
        command = hookCommands.promptPriors;
        agents = ["claude"];
      }
    ];

    PostToolUse = [
      # Arms the Stop review gate only after Claude edit tools changed files.
      {
        matcher = "Write|Edit|MultiEdit|NotebookEdit";
        command = hookCommands.reviewEditLogger;
        agents = ["claude"];
      }
      # Records each armed ScheduleWakeup fire time so the Stop gate below can
      # tell a dropped wakeup from a fired one (index#2259).
      {
        matcher = "ScheduleWakeup";
        command = hookCommands.wakeupArmLogger;
        agents = ["claude"];
      }
    ];

    Stop = [
      {
        command = hookCommands.stopReviewGate;
        agents = ["claude"];
      }
      # Retrospect a substantive session once per session (own marker), like
      # the review gate above — but out-of-band: the hook detaches a worker
      # that ships the transcript to the ix-mcp HTTP kernel (weave CAS blob)
      # and opens a `fabric.claude.session` agent there to file GitHub issues
      # for what was improvable. Stop is never blocked; without fleet creds
      # (IX_MCP_API_KEY[_FILE]) it fails open and does nothing.
      {
        command = hookCommands.stopRetroGate;
        agents = ["claude"];
      }
      # Blocks once when an armed ScheduleWakeup vanished before its fire time
      # (index#2259: pending wakeups are in-memory only and cleared by session
      # resume or user abort, so a drop is otherwise a silent stall).
      {
        command = hookCommands.stopWakeupGate;
        agents = ["claude"];
      }
      # Mines the session's transcript delta for friction, out-of-band like the
      # retro gate: a detached worker condenses the new delta locally (per-
      # session byte offset), ships it to the ix-mcp HTTP kernel (weave CAS
      # blob), and opens a `fabric.claude.session` agent to extract items, dedupe
      # against open issues in the Linear "Shitty" project, and file the new
      # ones. Nothing model- or credential-shaped runs locally; without fleet
      # creds (IX_MCP_API_KEY[_FILE]) it fails open and does nothing.
      {
        command = hookCommands.frictionIssueReporter;
      }
    ];

    SubagentStop = [
      {
        command = hookCommands.subagentCachePopulate;
        timeout = 30;
        agents = ["claude"];
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
  unique = lib.foldl' (acc: x:
    if lib.elem x acc
    then acc
    else acc ++ [x]) [];

  forAgent = agent: let
    groupsFor = hooks: let
      mine = builtins.filter (d: d.enable && lib.elem agent d.agents) hooks;
      group = matcher:
        {
          hooks = map (
            d:
              {
                type = "command";
                command = renderCommand d.command;
              }
              // lib.optionalAttrs (d.timeout != null) {inherit (d) timeout;}
          ) (builtins.filter (d: d.matcher == matcher) mine);
        }
        // lib.optionalAttrs (matcher != null) {inherit matcher;};
    in
      map group (unique (map (d: d.matcher) mine));
  in
    lib.filterAttrs (_: groups: groups != []) (lib.mapAttrs (_: groupsFor) withDefaults);
in {
  claude = forAgent "claude";
  codex = forAgent "codex";
}
