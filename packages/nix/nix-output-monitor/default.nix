{
  ix,
  lib,
  pkgs,
}: let
  inherit (pkgs.haskell.lib) compose;

  # `nom build` reads every .drv with the `nix-derivation` Haskell library. Its
  # 1.1.3 parser runs each output path through `filepathParser`, which fails on
  # an empty string, but a floating content-addressed (or deferred) output is
  # exactly `("out","","r:sha256","")` with an empty path. So nom spams
  # `DerivationParseError "string"` and renders no dependency graph for CA
  # derivations, which the index repo builds heavily. The fix is a registry
  # fork of Gabriella439/Haskell-Nix-Derivation-Library (nix-derivation-src +
  # ./patches, driven by lib/fork-packages.nix); the patch's commit message
  # carries the full WHY, including why it stays smaller than upstream PR #26.
  #
  # nixpkgs builds nom as `haskellPackages.callPackage ./generated-package.nix`
  # (top-level, not in the haskellPackages set), so feed the override through the
  # top-level package's `haskellPackages` argument rather than rebuilding the
  # by-name pipeline (postInstall symlinks, completions) here.
  #
  # Upstream: https://github.com/maralorn/nix-output-monitor/issues/122
  #           https://github.com/Gabriella439/Haskell-Nix-Derivation-Library/issues/28
  patchedNixDerivationSrc = ix.patchedSrc {
    name = "nix-derivation";
    src = ix.nix-derivationSrc;
    patchDir = ./patches;
  };

  # The hackage recipe expects its source's cabal version; a haskellPackages
  # bump past 1.1.3 with a stale nix-derivation-src pin would silently build
  # the old tree under the new label, so fail eval until the pin is advanced.
  # `overrideSrc` also drops the hackage cabal-revision overlay
  # (editedCabalFile), which is safe because the pinned tree already carries
  # the revisions' dependency-bound relaxations (see flake.nix).
  haskellPackages = pkgs.haskellPackages.extend (
    _hfinal: hprev: {
      nix-derivation = assert lib.assertMsg (hprev.nix-derivation.version == "1.1.3") ''
        packages/nix/nix-output-monitor: haskellPackages.nix-derivation is
        ${hprev.nix-derivation.version} but nix-derivation-src pins 1.1.3.
        Repin the nix-derivation-src input to the matching upstream rev and
        run `nix run .#rebase-patches -- nix-derivation`.'';
        compose.overrideSrc {
          src = patchedNixDerivationSrc;
          version = hprev.nix-derivation.version;
        }
        hprev.nix-derivation;
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
