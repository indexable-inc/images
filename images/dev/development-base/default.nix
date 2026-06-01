# Default ix dev base image: agent CLIs plus a normal build toolchain.
# The auto-enabled base profile (modules/profiles/base) supplies version
# control, editors, the nushell workspace wrapper, gdb/lldb, strace, tcpdump,
# jaq, btop, bpftrace, lsof, ncdu, pv, file, and the gnutar/gzip/zstd trio
# needed to stay `ix switch`-able.
{
  ix,
  lib,
  pkgs,
  ...
}:
let
  # Claude Code refuses bypass-permissions mode for the root user unless it is
  # told it is sandboxed. The shipped 2.x bundle guards both
  # `--dangerously-skip-permissions` and a settings.json
  # `permissions.defaultMode = "bypassPermissions"` behind
  # `getuid() === 0 && IS_SANDBOX !== "1" && !CLAUDE_CODE_BUBBLEWRAP`, and
  # otherwise exits with "cannot be used with root/sudo privileges". The agent
  # runs as root in this guest, so without the signal the bypass default below
  # is silently rejected. The guest VM is precisely the sandbox that guard asks
  # about, so bake IS_SANDBOX=1 into the claude binary. A wrapper (rather than a
  # global `environment.variables`) keeps the blast radius to claude and reaches
  # every launch path, including non-login `ssh root@vm -- claude` and `ix
  # shell` exec, which never source the login-shell environment. Named with the
  # upstream version so `lib.getName` stays "claude-code".
  claude-code =
    pkgs.runCommand "claude-code-${pkgs.claude-code.version}"
      { nativeBuildInputs = [ pkgs.makeWrapper ]; }
      ''
        makeWrapper ${pkgs.claude-code}/bin/claude "$out/bin/claude" --set IS_SANDBOX 1
      '';
in
{
  ix.image.name = "development-base";

  # `pkgs.claude-code` ships under Anthropic's commercial terms (unfree in
  # nixpkgs). Allow it by name so the exception is auditable and narrow;
  # codex is Apache-2.0 and does not need a predicate entry. Do not flip
  # `allowUnfree` to `true` globally: every other unfree package would slip
  # into this image unreviewed.
  nixpkgs.config.allowUnfreePredicate =
    pkg: builtins.elem (pkg.pname or (lib.getName pkg)) [ "claude-code" ];

  environment.systemPackages =
    builtins.attrValues {
      inherit (pkgs)
        # Coding agent. codex authenticates at first use inside the VM; no API
        # keys are baked into the image per the trust model. Claude Code rides
        # along below, wrapped for sandbox mode.
        codex

        # Browser automation for agents. `agent-browser` (vercel-labs) is the
        # CLI surface; `chromium` is the actual browser it drives. agent-browser
        # auto-detects a Chromium binary on PATH so no extra wiring is needed.
        # Kept local-only (no Browserbase / cloud provider) so sandboxes work
        # offline and don't need outbound API keys.
        agent-browser
        chromium

        # Build toolchain. Most ecosystems lean on cmake / make / ninja and
        # pkg-config; rustup keeps the toolchain pinnable per-project rather
        # than locking the image to one rustc.
        cmake
        gcc
        gnumake
        ninja
        pkg-config
        rustup

        # Default language runtimes that show up across most dev sessions.
        nodejs
        python3
        ;
    }
    ++ [
      # Claude Code, wrapped to advertise the sandbox (IS_SANDBOX=1) so the
      # bypass-permissions default below is honored for root. Same first-use
      # auth, no baked keys.
      claude-code
    ];

  # Claude Code policy for the dev image: enforce the bypass keys through
  # Claude's own managed-settings layer, not by writing the user's settings.json.
  #
  # This image only ever runs inside a per-tenant ix VM (or an `ix shell` user on
  # one), which is the real trust boundary: the agent can touch nothing but this
  # guest's disposable filesystem, network, and processes. Per-tool approval
  # prompts buy nothing here and only stall an agent that has nowhere unsafe to
  # go, so inside the guest we hand Claude full authority. The enforcement that
  # actually matters belongs to the sandbox that owns the guest, whether that is
  # the VM boundary or the OS user the agent runs as, not to a confirmation
  # dialog the agent answers itself.
  #
  # Claude Code reads `/etc/claude-code/managed-settings.json` as its
  # highest-precedence, enforced layer (above user, project, local, and CLI), and
  # only ever READS it. That makes a read-only /nix/store file (delivered by
  # environment.etc as an /etc symlink to the 0444 store copy) the right delivery:
  # no activation copy, no last-applied 3-way merge, no mutable generated file.
  # `~/.claude/settings.json` is left entirely app-owned, so Claude's in-app
  # settings pane can write theme/etc. with nothing for Nix to collide with. This
  # is the model #491 landed on after anna's note that layered, app-native config
  # (a read-only managed scope merged at load time) beats consumer-side merge
  # logic wherever the app provides it.
  #
  # Both keys must live in this managed file: `permissions.defaultMode =
  # "bypassPermissions"` runs every tool without a prompt, and
  # `skipDangerousModePermissionPrompt = true` pre-accepts the one-time bypass
  # warning, which managed bypass alone does not suppress. That key is ignored
  # only in *project* scope (a guard against untrusted repos), and is honored in
  # managed scope. Root additionally needs the IS_SANDBOX=1 signal from the
  # wrapped claude-code above (the uid-0 bypass guard rejects bypass mode for
  # root without it); the wrapper sets it unconditionally, so the guard is
  # satisfied however bypass is configured. Mirrors the intent of the ix fleet
  # default in ix's
  # nix/homes/modules/llm.nix (that module is per-user home-manager, so it can't
  # write /etc; moving the fleet onto a managed file is the follow-up).
  #
  # `env.CLAUDE_CODE_EXTRA_BODY` forces summarized thinking back on. Opus 4.7/4.8
  # silently changed the Messages API default for `thinking.display` from
  # "summarized" to "omitted": thinking blocks still stream, but their `thinking`
  # field is empty (only the encrypted `signature` is returned), so the agent's
  # reasoning is invisible in the transcript and Ctrl+O view. The in-app
  # `showThinkingSummaries` setting does NOT fix this — it controls the transcript
  # renderer, not the API request, and is never wired to the `display` param
  # (anthropics/claude-code#49268). The only restore path is to send
  # `thinking.display = "summarized"` on the request body, which Claude Code
  # merges from this env var.
  #
  # Why `type = "adaptive"` is here and why it's safe:
  #   - Claude Code parses CLAUDE_CODE_EXTRA_BODY and spreads it into the request
  #     body LAST, as a shallow top-level merge — so it REPLACES the whole
  #     `thinking` object rather than deep-merging into it. That means we must
  #     restate `type` here; sending `{ display = "summarized"; }` alone would
  #     drop `type` and the API would 400 (thinking requires a type).
  #   - `adaptive` is the only thinking type Opus 4.7/4.8 accept anyway — manual
  #     `type = "enabled"` with a budget returns 400 on these models — so this
  #     restates the harness's own default, it does not pick a narrower mode.
  #   - It does NOT pin the thinking level. The effort knob (low/medium/high/
  #     xhigh, set via /effort) is a SEPARATE top-level request field, a sibling
  #     of `thinking`, never a property inside it. Replacing `thinking` here only
  #     flips `display`; effort is untouched.
  #   - Caveat: this assumes an adaptive-capable model (Opus 4.6/4.7/4.8,
  #     Sonnet 4.6). Haiku 4.5 is manual-only and already defaults to summarized,
  #     so it neither needs nor accepts adaptive — fine while the image defaults
  #     to Opus.
  environment.etc."claude-code/managed-settings.json".text = builtins.toJSON {
    permissions.defaultMode = "bypassPermissions";
    skipDangerousModePermissionPrompt = true;
    env.CLAUDE_CODE_EXTRA_BODY = builtins.toJSON {
      thinking = {
        type = "adaptive";
        display = "summarized";
      };
    };
  };
}
