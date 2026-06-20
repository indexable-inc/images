# Single source of truth for agent lifecycle hooks, consumed by BOTH agent
# wrappers under ./ (claude-code, codex) — the hook analogue of common.nix
# (which shares systemPrompt + houseServers). Each hook is one subcommand of the
# compiled `claude-hooks` binary (packages/agent/claude-hooks); this module only
# DECLARES which subcommand runs on which event/matcher for which agent, then
# renders that one list into each agent's native hook-config shape.
#
# Imported from a wrapper's default.nix as
#   (import ../hooks.nix { inherit lib claudeHooks primaryCheckouts repoPackages; }).claude
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
  # Flake package set; `prompt-priors` is wired only when the `search` sibling is
  # in scope (it shells out to IX_SEARCH), matching the claude-code package.
  repoPackages ? { },
}:
let
  hookCmd = sub: "${claudeHooks}/bin/claude-hooks ${sub}";

  # One declaration per hook. Fields:
  #   event     hook event name (SessionStart, PreToolUse, PostToolUse, Stop, ...)
  #   sub       claude-hooks subcommand; the command is `<bin> <sub>`
  #   matcher   optional tool-name matcher (omit for always-run events)
  #   timeout   optional per-hook timeout (s); omit for the CLI default
  #   agents    which agents get it; defaults to both
  #   enable    optional bool gate (drops the declaration when false)
  declarations = [
    # SessionStart context: the pre-rendered fleet digest, plus the live host
    # banner + ~/Projects repo inventory. Both just add context; harmless on codex.
    {
      event = "SessionStart";
      sub = "session-digest";
      timeout = 5;
    }
    {
      event = "SessionStart";
      sub = "session-banner";
      timeout = 5;
    }

    # Score-gated ambient priors from the corpus store. Claude-only and gated on
    # the search sibling (it execs IX_SEARCH).
    {
      event = "UserPromptSubmit";
      sub = "prompt-priors";
      timeout = 5;
      agents = [ "claude" ];
      enable = repoPackages ? search;
    }

    # Deny edits whose target resolves into a protected primary checkout. The
    # Edit/Write matcher is claude-shaped (codex edits via apply_patch).
    {
      event = "PreToolUse";
      matcher = "Edit|MultiEdit|Write|NotebookEdit";
      sub = "worktree-guard";
      timeout = 10;
      agents = [ "claude" ];
      enable = primaryCheckouts != [ ];
    }

    # Bash guards: steer cargo to nix in the monorepos, and catch shell
    # anti-patterns. Codex's shell tool is matcher-aliased to "Bash", so both.
    {
      event = "PreToolUse";
      matcher = "Bash";
      sub = "cargo-guard";
    }
    {
      event = "PreToolUse";
      matcher = "Bash";
      sub = "bash-habits-guard";
    }

    # Deny the built-in Search tool (claude-only: codex has no Search tool).
    {
      event = "PreToolUse";
      matcher = "^Search$";
      sub = "search-guard";
      agents = [ "claude" ];
    }

    # Review gate pair: log edits, then require a review once per change-set on
    # Stop. Claude-only — codex edits via apply_patch, which the matcher never
    # sees, so the gate could never arm there.
    {
      event = "PostToolUse";
      matcher = "Write|Edit|MultiEdit|NotebookEdit";
      sub = "review-log-edit";
      agents = [ "claude" ];
    }
    {
      event = "Stop";
      sub = "review-gate";
      agents = [ "claude" ];
    }

    # Friction mining on every stop: analyze the transcript delta in the
    # background and file genuine friction to Linear. Reads both transcript
    # dialects, so both agents. Self-gates on the git author being an ix
    # contributor (compiled-in), replacing the old conditions/ix-contributor
    # wrapper.
    {
      event = "Stop";
      sub = "friction-report";
    }
  ];

  defaults = {
    matcher = null;
    timeout = null;
    agents = [
      "claude"
      "codex"
    ];
    enable = true;
  };

  withDefaults = map (d: defaults // d) declarations;

  unique = lib.foldl' (acc: x: if lib.elem x acc then acc else acc ++ [ x ]) [ ];

  # Render the declaration list into the settings.json/codex `hooks` attrset for
  # one agent: { <Event> = [ { matcher?; hooks = [ { type; command; timeout?; } ]; } ]; }.
  forAgent =
    agent:
    let
      mine = builtins.filter (d: d.enable && lib.elem agent d.agents) withDefaults;
      events = unique (map (d: d.event) mine);
      groupsFor =
        event:
        let
          ofEvent = builtins.filter (d: d.event == event) mine;
          group =
            matcher:
            {
              hooks = map (
                d:
                {
                  type = "command";
                  command = hookCmd d.sub;
                }
                // lib.optionalAttrs (d.timeout != null) { inherit (d) timeout; }
              ) (builtins.filter (d: d.matcher == matcher) ofEvent);
            }
            // lib.optionalAttrs (matcher != null) { inherit matcher; };
        in
        map group (unique (map (d: d.matcher) ofEvent));
    in
    lib.genAttrs events groupsFor;
in
{
  claude = forAgent "claude";
  codex = forAgent "codex";
}
