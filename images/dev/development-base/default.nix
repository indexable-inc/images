# Default ix dev base image: agent CLIs plus a normal build toolchain.
# The auto-enabled base profile (modules/profiles/base) supplies version
# control, editors, the nushell workspace wrapper, gdb/lldb, strace, tcpdump,
# jq, btop, bpftrace, lsof, ncdu, pv, file, and the gnutar/gzip/zstd trio
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

  # Claude Code defaults for the dev image: a writable, Nix-generated
  # settings.json.
  #
  # This image only ever runs inside a per-tenant ix VM (or an `ix shell` user
  # on one), which is the real trust boundary: the agent can touch nothing but
  # this guest's disposable filesystem, network, and processes. Per-tool
  # approval prompts buy nothing here and only stall an agent that has nowhere
  # unsafe to go, so inside the guest we hand Claude full authority. The
  # enforcement that actually matters belongs to the sandbox that owns the
  # guest, whether that is the VM boundary or the OS user the agent runs as,
  # not to a confirmation dialog the agent answers itself.
  #
  # `permissions.defaultMode = "bypassPermissions"` runs every tool without an
  # approval prompt; `skipDangerousModePermissionPrompt` pre-accepts the
  # one-time bypass-mode warning so a fresh interactive launch does not stall on
  # the confirmation dialog (both honored only in user-scope settings.json,
  # which this is). Mirrors the ix fleet default in ix's
  # nix/homes/modules/llm.nix. Root additionally needs the IS_SANDBOX=1 signal
  # from the wrapped claude-code above, or this mode is rejected.
  #
  # Delivered as a WRITABLE file, not a read-only /nix/store symlink (what
  # home.file and home-manager's programs.claude-code emit): Claude's in-app
  # settings pane writes back to settings.json, and other agents (Codex, ...)
  # rewrite their own config too, so a read-only symlink would make every such
  # write fail with a permission error. The mutable-json module keeps it both
  # Nix-declared (so the defaults stay composable, unlike mkOutOfStoreSymlink's
  # raw file) and writable, reconciling with a last-applied 3-way merge on each
  # switch: our keys are enforced, the app's own keys are preserved. See
  # lib/mutable-json.nix.
  home-manager.users.root = {
    imports = [ ix.mutableJson.homeModule ];
    home.mutableJsonFiles.claude-code = {
      target = ".claude/settings.json";
      value = {
        "$schema" = "https://json.schemastore.org/claude-code-settings.json";
        permissions.defaultMode = "bypassPermissions";
        skipDangerousModePermissionPrompt = true;
      };
    };
  };
}
