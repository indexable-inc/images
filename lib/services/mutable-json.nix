# Declarative-but-writable JSON config files: the fallback for app config that
# Nix can't deliver read-only.
#
# PREFER the app's own managed/policy layer for any key you want to ENFORCE.
# Many apps already merge several config layers at load time with a read-only,
# highest-precedence "managed" scope on top: Claude Code reads
# /etc/claude-code/managed-settings.json, Codex reads /etc/codex/requirements.toml,
# Firefox has managed policies, etc. Enforcing a key there needs no merge logic
# and no mutable generated file: ship a read-only /nix/store file and leave the
# app's own config fully app-owned. That is the right tool whenever the app
# provides it (see issue #491 and lib/dev/base, which enforces
# Claude's bypass keys through the managed layer rather than this module).
#
# This module is for what's left: a key that must live in a file the app ALSO
# writes to and that has no managed layer, or a value you want to SEED as an
# overridable default rather than enforce (a managed layer always wins, so it
# can't express "default the user may change"). For those, a read-only
# `/nix/store` symlink (`home.file`) makes the app's own writes fail with a
# permission error, and the usual escape hatches each give something up:
# `mkOutOfStoreSymlink` hands you a raw working-copy file with no Nix algebra
# (no merge of contributions across modules, no types), and a plain
# activation-copy overwrites the app's runtime writes on every switch.
#
# This module keeps the file writable AND Nix-declared by reconciling with a
# last-applied 3-way merge (the kubectl-apply model): on activation it computes
#
#   result = deepmerge( prune(live, last \ new), new )
#
# so Nix enforces the keys it declares, prunes keys it stops declaring, and
# preserves everything the app wrote itself. The previous declaration is recorded
# under `xdg.stateHome` so "what Nix used to own" is known across switches. The
# merge itself lives in the sidecar `mutable-json-merge.jq`.
#
# Scope: a single declarative owner per file. Multiple Nix-side owners of one
# file would need per-field ownership (Server-Side Apply), which this is not.
#
# Exposed from the flake as `homeModules.mutable-json`; see `tests/` and the
# `mutable-json-merge` flake check for the merge cases.
{lib}: let
  inherit (lib) types mkOption;

  homeModule = {
    config,
    lib,
    pkgs,
    ...
  }: let
    cfg = config.home.mutableJsonFiles;
    jsonFormat = pkgs.formats.json {};
    mergeProgram = ./mutable-json-merge.jq;
    stateDir = "${config.xdg.stateHome}/home-manager/mutable-json";

    resolveTarget = target:
      if lib.hasPrefix "/" target
      then target
      else "${config.home.homeDirectory}/${target}";

    # One reconcile step: deep-merge the Nix-declared `value` over the live file
    # (3-way against the recorded last-applied), write it back atomically, then
    # record the new declaration as the next last-applied. Store paths are
    # interpolated raw; caller-influenced paths go through escapeShellArg.
    mkReconcile = name: entry: let
      target = resolveTarget entry.target;
      desired = jsonFormat.generate "mutable-json-${name}.json" entry.value;
      # One state file per target, keyed by a hash so odd paths stay safe.
      state = "${stateDir}/${builtins.hashString "sha256" target}.json";
      targetArg = lib.escapeShellArg target;
      stateArg = lib.escapeShellArg state;
    in ''
      ${pkgs.coreutils}/bin/mkdir -p "$(${pkgs.coreutils}/bin/dirname ${targetArg})" ${lib.escapeShellArg stateDir}
      # An absent (or unreadable) target/state file is treated as empty `{}`;
      # malformed JSON in either makes jq fail below and aborts the switch
      # rather than silently overwriting.
      _live=$([ -f ${targetArg} ] && ${pkgs.coreutils}/bin/cat ${targetArg} || ${pkgs.coreutils}/bin/echo '{}')
      _last=$([ -f ${stateArg} ] && ${pkgs.coreutils}/bin/cat ${stateArg} || ${pkgs.coreutils}/bin/echo '{}')
      _merged=$(${pkgs.jq}/bin/jq -n \
        --argjson last "$_last" \
        --argjson live "$_live" \
        --argjson new "$(${pkgs.coreutils}/bin/cat ${desired})" \
        -f ${mergeProgram})
      ${pkgs.coreutils}/bin/printf '%s\n' "$_merged" > ${targetArg}.hm-mutable-json-tmp
      ${pkgs.coreutils}/bin/mv -f ${targetArg}.hm-mutable-json-tmp ${targetArg}
      ${pkgs.coreutils}/bin/install -m644 ${desired} ${stateArg}
    '';
  in {
    options.home.mutableJsonFiles = mkOption {
      default = {};
      description = ''
        JSON config files that Nix declares but the owning app may also write
        to at runtime. Each entry's `value` is reconciled into a writable
        `target` with a last-applied 3-way merge on activation: declared keys
        are enforced, keys Nix stops declaring are pruned, and the app's own
        keys are preserved.
      '';
      type = types.attrsOf (
        types.submodule {
          options = {
            target = mkOption {
              type = types.str;
              example = ".claude/settings.json";
              description = "Destination file, absolute or relative to the home directory.";
            };
            value = mkOption {
              inherit (jsonFormat) type;
              description = "The keys Nix owns. Rendered to JSON and merged into `target`.";
            };
          };
        }
      );
    };

    config = lib.mkIf (cfg != {}) {
      home.activation.mutableJsonFiles = lib.hm.dag.entryAfter ["writeBoundary"] (
        lib.concatStringsSep "\n" (lib.mapAttrsToList mkReconcile cfg)
      );
    };
  };
in {
  inherit homeModule;
  # The merge program, exposed so tests can exercise it directly on fixtures.
  mergeProgram = ./mutable-json-merge.jq;
}
