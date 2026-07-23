{
  ix,
  lib,
}: let
  # nixpkgs' statix with its vendored rnix patched to lex underscore digit
  # separators in numeric literals (lib/util/rnix-digit-separators), so the
  # linter accepts the dialect the patched nix (nix-ix) parses.
  # statix reports style lints; the sample stays lint-free so the exit code isolates the parse.
  # Injected as `pkgs.statix` via the overlay so the repo lint gate and dev
  # shells lint the same language the fleet evaluates.
  #
  # Built from nixpkgs' own by-name definition rather than `pkgs.statix`:
  # this package IS the overlay's `statix`, so reading the bare attr would
  # make it its own base (the same recursion packages/nix documents).
  inherit (ix) pkgs;

  packagePath = pkgs.path + "/pkgs/by-name/st/statix/package.nix";
  base = assert lib.assertMsg (builtins.pathExists packagePath)
  "packages/statix: nixpkgs moved statix out of pkgs/by-name; update packagePath";
    pkgs.callPackage packagePath {};

  package = ix.rnixDigitSeparators base;

  sample = pkgs.writeText "underscore-literals.nix" ''
    {
      budget = 8_000_000_000;
      epsilon = 0.000_001;
      plain = 1000.5;
      port = 10_000;
      scale = 2.5e1_0;
    }
  '';

  # The override's real risk is the patched tokenizer rejecting the new
  # literals as a parse error, so the smoke test lints a clean file that
  # uses them and requires a zero exit.
  smoke =
    pkgs.runCommand "statix-underscore-smoke"
    {
      nativeBuildInputs = [package];
      strictDeps = true;
    }
    ''
      mkdir sample
      install -m 644 ${sample} sample/underscore-literals.nix
      statix check sample
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
