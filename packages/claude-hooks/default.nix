# Shared agent hook scripts, packaged once so every Claude Code / Codex
# consumer (the claude-code wrapper, the ix repo, personal home configs) runs
# the same guards instead of carrying drifting copies. The scripts came from
# ix/.claude/hooks and the personal nix repo's dots/claude/hooks, which had
# already diverged; this package is the single source of truth.
#
# The directory layout is flat (`<out>/<hook>.sh`) so a consumer can symlink
# the whole store path to `~/.claude/hooks` or point a settings.json hook
# command at one file. `passthru.settingsFragment` carries the ready-made
# settings.json `hooks` stanza wired to these store paths, which the
# claude-code package bakes into its flagSettings layer.
{
  lib,
  pkgs,
}:

pkgs.stdenvNoCC.mkDerivation (finalAttrs: {
  name = "claude-hooks";

  dontUnpack = true;

  # python3 resolves the `#!/usr/bin/env python3` shebangs of the two guard
  # scripts at build time; makeWrapper pins the shell hooks' runtime tools.
  nativeBuildInputs = [
    pkgs.makeWrapper
    pkgs.python3
  ];
  strictDeps = true;

  installPhase = ''
    runHook preInstall

    mkdir -p "$out"
    install -m 0755 ${./hooks}/* "$out"/
    # --build: under strictDeps the default host mode resolves interpreters
    # against HOST_PATH, where nativeBuildInputs (python3 here) never appear,
    # silently leaving `#!/usr/bin/env python3` unpatched. Build mode resolves
    # against PATH, which is where python3 actually is; for this
    # run-on-the-host script package the two platforms are the same.
    patchShebangs --build "$out"

    # The shell hooks call jq and git; pin them so the hook works under the
    # minimal PATH of a hook invocation. Prefix (not set) so `hostname`, which
    # has no portable nixpkgs package on darwin, still resolves from the host.
    for hook in session-start.sh session-id.sh; do
      wrapProgram "$out/$hook" \
        --prefix PATH : ${
          lib.makeBinPath [
            pkgs.git
            pkgs.jq
          ]
        }
    done

    runHook postInstall
  '';

  passthru.settingsFragment = {
    SessionStart = [
      {
        hooks = [
          {
            type = "command";
            command = "${finalAttrs.finalPackage}/session-start.sh";
          }
        ];
      }
      {
        hooks = [
          {
            type = "command";
            command = "${finalAttrs.finalPackage}/session-id.sh";
            timeout = 5;
          }
        ];
      }
    ];
    PreToolUse = [
      {
        matcher = "Bash";
        hooks = [
          {
            type = "command";
            command = "${finalAttrs.finalPackage}/enforce-modern-tools.sh";
          }
        ];
      }
      {
        matcher = "Bash";
        hooks = [
          {
            type = "command";
            command = "${finalAttrs.finalPackage}/block-test-output-filtering.sh";
          }
        ];
      }
    ];
  };

  meta = {
    description = "Shared Claude Code / Codex hook scripts plus their settings.json hooks stanza";
    license = lib.licenses.mit;
  };
})
