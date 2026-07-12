{
  ix,
  lib,
}: let
  # nixpkgs' alejandra with its vendored rnix patched to lex underscore digit
  # separators in numeric literals (lib/util/rnix-digit-separators), so the
  # formatter accepts the dialect the patched nix (nix-ix) parses and passes
  # the separators through verbatim. Injected as `pkgs.alejandra` via the
  # overlay so every consumer -- the repo lint gate, dev shells, editor
  # format-on-save -- formats the same language the fleet evaluates.
  #
  # Built from nixpkgs' own by-name definition rather than `pkgs.alejandra`:
  # this package IS the overlay's `alejandra`, so reading the bare attr would
  # make it its own base (the same recursion packages/nix/nix documents).
  inherit (ix) pkgs;

  packagePath = pkgs.path + "/pkgs/by-name/al/alejandra/package.nix";
  base = assert lib.assertMsg (builtins.pathExists packagePath)
  "packages/nix/alejandra: nixpkgs moved alejandra out of pkgs/by-name; update packagePath";
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

  # The override's real risk is the patched tokenizer rejecting or mangling
  # the new literals, so the smoke test formats a file that uses them and
  # asserts every separator survives verbatim (a formatter must never rewrite
  # token text) alongside an untouched classic literal.
  smoke =
    pkgs.runCommand "alejandra-underscore-smoke"
    {
      nativeBuildInputs = [package];
      strictDeps = true;
    }
    ''
      install -m 644 ${sample} sample.nix
      alejandra --quiet sample.nix
      for literal in 8_000_000_000 0.000_001 1000.5 10_000 2.5e1_0; do
        grep -F "$literal" sample.nix >/dev/null || {
          echo "alejandra dropped or rewrote the literal $literal" >&2
          cat sample.nix >&2
          exit 1
        }
      done
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
