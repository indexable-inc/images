{pkgs}: let
  inherit (pkgs.haskell.lib) compose;

  # `nom build` reads every .drv with the `nix-derivation` Haskell library. Its
  # 1.1.3 parser runs each output path through `filepathParser`, which fails on
  # an empty string, but a floating content-addressed (or deferred) output is
  # exactly `("out","","r:sha256","")` with an empty path. So nom spams
  # `DerivationParseError "string"` (attoparsec's `string` combinator failing on
  # the next `]`/`,` literal after the output parser backtracks) and renders no
  # dependency graph for CA derivations, which the index repo builds heavily.
  #
  # The patch keeps the single-constructor `DerivationOutput` record and only
  # widens `filepathParser` to accept an empty path. nom is then unchanged:
  # `insertDerivation` calls `parseStorePath ""`, which returns `Nothing`, so the
  # still-unrealised floating output is dropped via `traverseMaybeWithKey`
  # instead of crashing on a partial field selector. This is deliberately
  # smaller than upstream PR #26, which turns `DerivationOutput` into a sum type
  # and would force a matching nom source patch.
  #
  # nixpkgs builds nom as `haskellPackages.callPackage ./generated-package.nix`
  # (top-level, not in the haskellPackages set), so feed the override through the
  # top-level package's `haskellPackages` argument rather than rebuilding the
  # by-name pipeline (postInstall symlinks, completions) here.
  #
  # Upstream: https://github.com/maralorn/nix-output-monitor/issues/122
  #           https://github.com/Gabriella439/Haskell-Nix-Derivation-Library/issues/28
  haskellPackages = pkgs.haskellPackages.extend (
    _hfinal: hprev: {
      nix-derivation = compose.appendPatch ./ca-empty-output-path.patch hprev.nix-derivation;
    }
  );

  package = pkgs.callPackage (pkgs.path + "/pkgs/by-name/ni/nix-output-monitor/package.nix") {
    inherit haskellPackages;
  };

  # The override's real risk is the Haskell rebuild linking at all, so the smoke
  # test runs the binary. `nom --help` exits 0 and prints usage without spawning
  # `nix` (`--version` shells out to `nix`, absent in the sandbox).
  smoke =
    pkgs.runCommand "nix-output-monitor-smoke"
    {
      nativeBuildInputs = [package];
      strictDeps = true;
    }
    ''
      help=$(nom --help 2>&1) || true
      case "$help" in
        *"nix-output-monitor usages"*) ;;
        *)
          echo "nom --help did not print usage" >&2
          printf '%s\n' "$help" >&2
          exit 1
          ;;
      esac
      mkdir -p "$out"
    '';
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests = {
          inherit smoke;
        };
      };
    meta =
      (old.meta or {})
      // {
        description = "nix-output-monitor with nix-derivation patched to parse content-addressed derivations";
        mainProgram = "nom";
      };
  })
