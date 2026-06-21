# Shared lifecycle hook declarations for Claude Code and Codex wrappers.
{
  lib,
  hookRunner,
  primaryCheckouts ? [ ],
  personalStartupContext ? false,
}:
let
  hookRunnerSubcommand = sub: {
    package = hookRunner;
    exeName = "claude-hooks";
    args = [ sub ];
  };

  hookCommands = {
    cachedStartupNotes = hookRunnerSubcommand "session-digest";
    hostInventoryBanner = hookRunnerSubcommand "session-banner";
    protectedCheckoutGuard = hookRunnerSubcommand "worktree-guard";
    nixCargoGuard = hookRunnerSubcommand "cargo-guard";
    shellHabitGuard = hookRunnerSubcommand "bash-habits-guard";
    indexedSearchGuard = hookRunnerSubcommand "search-guard";
    promptPriors = hookRunnerSubcommand "prompt-priors";
    subagentCacheLookup = hookRunnerSubcommand "subagent-cache-lookup";
    reviewEditLogger = hookRunnerSubcommand "review-log-edit";
    stopReviewGate = hookRunnerSubcommand "review-gate";
    frictionIssueReporter = hookRunnerSubcommand "friction-report";
    subagentCachePopulate = hookRunnerSubcommand "subagent-cache-populate";
  };

  renderCommand =
    command:
    lib.escapeShellArgs (
      [
        (lib.getExe' command.package command.exeName)
      ]
      ++ command.args
    );

  declarations = {
    SessionStart = [
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
        agents = [ "claude" ];
        enable = primaryCheckouts != [ ];
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
        agents = [ "claude" ];
      }
      {
        matcher = "Agent";
        command = hookCommands.subagentCacheLookup;
        timeout = 15;
        agents = [ "claude" ];
      }
    ];

    UserPromptSubmit = [
      {
        command = hookCommands.promptPriors;
        agents = [ "claude" ];
      }
    ];

    PostToolUse = [
      # Arms the Stop review gate only after Claude edit tools changed files.
      {
        matcher = "Write|Edit|MultiEdit|NotebookEdit";
        command = hookCommands.reviewEditLogger;
        agents = [ "claude" ];
      }
    ];

    Stop = [
      {
        command = hookCommands.stopReviewGate;
        agents = [ "claude" ];
      }
      {
        command = hookCommands.frictionIssueReporter;
      }
    ];

    SubagentStop = [
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
