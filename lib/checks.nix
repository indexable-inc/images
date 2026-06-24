{
  lib,
}:
{
  mkNixUnitSuites =
    {
      pkgs,
      nixUnit,
      src,
      suiteTiers,
      entryFor ? (
        tier: name:
        pkgs.writeText "nix-unit-entry-${tier}-${name}.nix" ''
          import ${src}/checks/${tier}/${name}.nix {
            lib = import ${pkgs.path + "/lib"};
          }
        ''
      ),
      resultPrefix ? "eval",
    }:
    let
      mkCheck =
        tier: name:
        pkgs.runCommand "${resultPrefix}-${tier}-${name}" { nativeBuildInputs = [ nixUnit ]; } ''
          export HOME="$(mktemp -d)"
          export NIX_STORE_DIR=${builtins.storeDir}
          export NIX_REMOTE="$HOME/store"
          nix-unit --eval-store "$HOME/store" --gc-roots-dir "$HOME/gc" ${entryFor tier name}
          mkdir -p "$out"
          printf 'nix-unit ok: %s/%s\n' ${lib.escapeShellArg tier} ${lib.escapeShellArg name} > "$out/result"
        '';

      byTier = lib.mapAttrs (tier: names: lib.genAttrs names (mkCheck tier)) suiteTiers;
    in
    {
      flat = lib.concatMapAttrs (
        tier: byName:
        lib.mapAttrs' (name: drv: lib.nameValuePair "${resultPrefix}-${tier}-${name}" drv) byName
      ) byTier;
    };

  mkBooleanCheck =
    {
      pkgs,
      prefix,
    }:
    name:
    {
      ok,
      successMessage ? "${prefix} ok: ${name}",
      failureMessage,
    }:
    pkgs.runCommand "${prefix}-${name}" { __structuredAttrs = true; } (
      if ok then
        ''
          mkdir -p "$out"
          printf '%s\n' ${lib.escapeShellArg successMessage} > "$out/result"
        ''
      else
        ''
          printf '%s\n' ${lib.escapeShellArg failureMessage} >&2
          exit 1
        ''
    );

  mkScriptCheck =
    {
      pkgs,
      prefix,
      nativeBuildInputs ? [ ],
    }:
    name: script:
    pkgs.runCommand "${prefix}-${name}"
      {
        __structuredAttrs = true;
        inherit nativeBuildInputs;
      }
      ''
        ${script}
        mkdir -p "$out"
        printf '${prefix} ok: %s\n' ${lib.escapeShellArg name} > "$out/result"
      '';
}
