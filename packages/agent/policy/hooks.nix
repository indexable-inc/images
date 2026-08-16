# Shared lifecycle hook declarations for Claude Code and Codex wrappers.
{
  lib,
  hookRunner,
  primaryCheckouts ? [],
  personalStartupContext ? false,
  # Off by default: Opus 5 reaches for the review agent more than the work
  # warrants, so an always-armed Stop gate turns a one-line fix into a
  # four-agent fan-out. Opt in per consumer if you want the gate back.
  alwaysOnReview ? false,
  # Consumer-supplied SessionStart context commands: each entry
  # ({ package, exeName ? null, args ? [], timeout ? 10 }) runs at session
  # start and its stdout is injected as session context. The generic seam
  # personal memory/notes wiring hangs off (index#3849).
  extraSessionStart ? [],
  # Mirrors `policy/permissions.nix`'s gate of the same name: when the consumer
  # puts the agent in strict kernel-only mode, arm the PreToolUse hook that
  # makes it exhaustive. The deny list in permissions.nix strips the schemas of
  # the tools this repo can name; this hook is what covers the ones it cannot,
  # and the only seam that can hand the agent a message saying where to go
  # instead. Default false: unarmed, the hook never runs and costs nothing.
  kernelOnly ? false,
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
    proseDashGuard = hookRunnerSubcommand "prose-dash-guard";
    destructiveGitGuard = hookRunnerSubcommand "git-guard";
    protectedCheckoutBashGuard = hookRunnerSubcommand "write-guard";
    indexedSearchGuard = hookRunnerSubcommand "search-guard";
    kernelOnlyGuard = hookRunnerSubcommand "kernel-only-guard";
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
        (
          if command.exeName or null != null
          then lib.getExe' command.package command.exeName
          else lib.getExe command.package
        )
      ]
      ++ command.args
    );

  declarations = {
    SessionStart =
      [
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
      ]
      ++ map (extra: {
        command = {
          inherit (extra) package;
          exeName = extra.exeName or null;
          args = extra.args or [];
        };
        timeout = extra.timeout or 10;
      })
      extraSessionStart;

    PreToolUse = [
      # Strict mode, first in the list and matcher-less so it sees EVERY tool
      # call: allowlist the index kernel, deny the rest with an instruction to
      # use it. Armed for both agents -- the codex fork honors the same house
      # JSON deny (see claude-hooks/src/guards.rs), which matters more there
      # than here, since codex's `apply_patch` has no config leaf to disable
      # and this hook is the only thing that reaches it.
      {
        command = hookCommands.kernelOnlyGuard;
        timeout = 10;
        enable = kernelOnly;
      }
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
      # The no-dash rule is the most frequently applied line in the house
      # prompt, and holding it in mind on every clause is attention spent on
      # every response. Where the prose lands in a `git`/`gh` argument the
      # character can be checked instead of remembered. Chat text has no such
      # seam: no hook can rewrite a reply, and a Stop gate would argue about a
      # dash the user has already read.
      {
        matcher = "Bash";
        command = hookCommands.proseDashGuard;
      }
      # git has no pre-reset/pre-checkout/pre-clean/pre-stash hook, so the only
      # seam that sees `git reset --hard` before it runs is PreToolUse
      # (ENG-9964). Same protected list as the edit guard above: without one
      # there is nothing to protect, so it is not installed. The edit guard
      # judges Edit/Write target paths and never sees Bash, so this is also the
      # only thing standing between an agent and `git add`/`git switch` in a
      # shared checkout (index#4218).
      {
        matcher = "Bash";
        command = hookCommands.destructiveGitGuard;
        timeout = 10;
        enable = primaryCheckouts != [];
      }
      # The same protected list again, for the writes that reach a shared
      # checkout without going through git or through a typed edit tool. The
      # edit guard above judges `file_path` and never sees Bash, so a subagent
      # holding only `Bash` wrote a module, a doc and a 55-line option into the
      # primary checkout through heredoc redirects, `cp` and `rm` (index#4310).
      {
        matcher = "Bash";
        command = hookCommands.protectedCheckoutBashGuard;
        timeout = 10;
        enable = primaryCheckouts != [];
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
        enable = alwaysOnReview;
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
        enable = alwaysOnReview;
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
