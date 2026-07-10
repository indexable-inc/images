/**
The agent CLI layer: Claude Code and Codex, gated on `ix.dev.agents.*`.

Single source of truth for "our versions of the agents." The dev base module
and `index.lib.mkDev` import this module, so the wrapped `claude` binary and
its managed-settings policy are defined once and cannot drift between a base
environment and a dev fleet. Importing the module twice is idempotent: it is
one module path, so there is exactly one wrapped `claude`, no `bin/claude`
collision.

Defaults are on, so a dev fleet ships both agents from a plain import; a fork
turns one off with `ix.dev.agents.codex = false;`.
*/
{
  lib,
  pkgs,
  config,
  ...
}: let
  cfg = config.ix.dev.agents;

  # Claude Code refuses bypass-permissions mode for the root user unless it is
  # told it is sandboxed. The shipped 2.x bundle guards both
  # `--dangerously-skip-permissions` and a settings.json
  # `permissions.defaultMode = "bypassPermissions"` behind
  # `getuid() === 0 && IS_SANDBOX !== "1" && !CLAUDE_CODE_BUBBLEWRAP`, and
  # otherwise exits with "cannot be used with root/sudo privileges". The agent
  # runs as root in the guest, so without the signal the bypass default below is
  # silently rejected. The guest VM is precisely the sandbox that guard asks
  # about, so bake IS_SANDBOX=1 into the claude binary. A wrapper (rather than a
  # global `environment.variables`) keeps the blast radius to claude and reaches
  # every launch path, including non-login `ssh root@vm -- claude` and `ix
  # shell` exec, which never source the login-shell environment. Named with the
  # upstream version so `lib.getName` stays "claude-code".
  claude-code =
    pkgs.runCommand "claude-code-${pkgs.claude-code.version}"
    {nativeBuildInputs = [pkgs.makeWrapper];}
    ''
      makeWrapper ${pkgs.claude-code}/bin/claude "$out/bin/claude" --set IS_SANDBOX 1
    '';
in {
  imports = [./options.nix];

  config = lib.mkMerge [
    (lib.mkIf cfg.codex {
      # codex is Apache-2.0; no allowUnfree exception needed. It authenticates
      # at first use inside the VM, so no API keys are baked into the image.
      environment.systemPackages = [pkgs.codex];
    })

    (lib.mkIf cfg.claude {
      # `pkgs.claude-code` is unfree; the allow-by-name exception lives on the
      # shared image nixpkgs instance (lib/image/default.nix), not here.
      environment.systemPackages = [claude-code];

      # Claude Code policy for dev images: enforce the bypass keys through
      # Claude's own managed-settings layer, not by writing the user's
      # settings.json. A dev image only ever runs inside a per-tenant ix VM (the
      # real trust boundary), so per-tool approval prompts buy nothing and only
      # stall an agent that has nowhere unsafe to go.
      #
      # Claude Code reads `/etc/claude-code/managed-settings.json` as its
      # highest-precedence, enforced layer and only ever READS it, so a
      # read-only /nix/store file delivered by environment.etc is the right
      # shape: no activation copy, no merge, no mutable generated file.
      # `~/.claude/settings.json` is left entirely app-owned - which is also why
      # binding `~/.claude` onto a shared volume (mkDev's `shared.claude`) does
      # not collide with this managed layer.
      #
      # Both keys must live here: `permissions.defaultMode = "bypassPermissions"`
      # runs every tool without a prompt, and `skipDangerousModePermissionPrompt
      # = true` pre-accepts the one-time bypass warning, which managed bypass
      # alone does not suppress (that key is ignored only in *project* scope, and
      # honored in managed scope).
      #
      # `env.CLAUDE_CODE_EXTRA_BODY` forces summarized thinking back on: Opus
      # 4.7/4.8 silently changed the Messages API default for `thinking.display`
      # from "summarized" to "omitted", so the agent's reasoning is invisible in
      # the transcript without it. Claude Code spreads this env var into the
      # request body as a shallow top-level merge, REPLACING the whole `thinking`
      # object, so `type` must be restated; `adaptive` is the only thinking type
      # Opus 4.7/4.8 accept and is the harness's own default, so this does not
      # pin the thinking level (the effort knob is a separate request field).
      environment.etc."claude-code/managed-settings.json".text = builtins.toJSON {
        permissions.defaultMode = "bypassPermissions";
        skipDangerousModePermissionPrompt = true;
        env.CLAUDE_CODE_EXTRA_BODY = builtins.toJSON {
          thinking = {
            type = "adaptive";
            display = "summarized";
          };
        };
        # Keep session transcripts effectively forever (~2700 years). Claude Code
        # otherwise deletes `~/.claude/projects/**/*.jsonl` after
        # `cleanupPeriodDays` (default 30), the only on-disk record of a run's
        # prompts, tool calls, and reasoning. Retention is free - local JSONL on
        # the guest disk - so we pay disk rather than lose the audit trail.
        cleanupPeriodDays = 999999;
      };
    })
  ];
}
