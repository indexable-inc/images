{
  ix,
  lib,
}: let
  # nixpkgs' deadnix with its vendored rnix patched to lex underscore digit
  # separators in numeric literals (lib/util/rnix-digit-separators), so the
  # linter accepts the dialect the patched nix (nix-ix) parses.
  # deadnix walks bindings for liveness; the sample uses its binding so the exit code isolates the parse.
  # Injected as `pkgs.deadnix` via the overlay so the repo lint gate and dev
  # shells lint the same language the fleet evaluates.
  #
  # Built from nixpkgs' own by-name definition rather than `pkgs.deadnix`:
  # this package IS the overlay's `deadnix`, so reading the bare attr would
  # make it its own base (the same recursion packages/nix/nix documents).
  inherit (ix) pkgs;

  packagePath = pkgs.path + "/pkgs/by-name/de/deadnix/package.nix";
  base = assert lib.assertMsg (builtins.pathExists packagePath)
  "packages/nix/deadnix: nixpkgs moved deadnix out of pkgs/by-name; update packagePath";
    pkgs.callPackage packagePath {};

  package = ix.rnixDigitSeparators base;

  sample = pkgs.writeText "underscore-literals.nix" ''
    let
      port = 10_000;
    in
      port + 8_000_000_000 + 2.5e1_0 + 0.000_001
  '';

  # The override's real risk is the patched tokenizer rejecting the new
  # literals as a parse error, so the smoke test lints a clean file that
  # uses them and requires a zero exit.
  smoke =
    pkgs.runCommand "deadnix-underscore-smoke"
    {
      nativeBuildInputs = [package];
      strictDeps = true;
    }
    ''
      install -m 644 ${sample} underscore-literals.nix
      deadnix --fail underscore-literals.nix
      mkdir -p "$out"
    '';
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests =
          (old.passthru.tests or {})
          // {
            inherit smoke;
          };
      };
  })
