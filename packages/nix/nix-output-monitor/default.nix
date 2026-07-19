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
  overrideNixDerivation = hprev:
    assert lib.assertMsg (hprev.nix-derivation.version == "1.1.3") ''
      packages/nix/nix-output-monitor: haskellPackages.nix-derivation is
      ${hprev.nix-derivation.version} but nix-derivation-src pins 1.1.3.
      Repin the nix-derivation-src input to the matching upstream rev and
      run `nix run .#rebase-patches -- nix-derivation`.'';
      compose.overrideSrc {
        src = patchedNixDerivationSrc;
        version = hprev.nix-derivation.version;
      }
      hprev.nix-derivation;

  haskellPackages = pkgs.haskellPackages.extend (
    _hfinal: hprev: {
      nix-derivation = overrideNixDerivation hprev;
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

  nativeNom = package.overrideAttrs (old: {
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
  });

  # Linux->Darwin cross build (#3606): nom is Haskell, so unlike btop (#3584)
  # the toolchain alone is not enough -- this lane rides ix.crossGhc, a
  # Linux-hosted GHC targeting Darwin built against the same apple-sdk clang
  # toolchain, and ix.crossHaskell, which compiles nom 2.1.8's 62-package
  # closure with it (rung 0 of #3606 verified the closure executes no Template
  # Haskell splices, so no Darwin iserv is needed). The same registry-fork
  # nix-derivation source applies via srcFor, so a Mac substituting this
  # output parses this repo's CA derivations exactly like a native build.
  crossNom = let
    inherit (ix.cross) target;
    toolchain = ix.appleSdkToolchain {
      appleSdk = ix.macosSdk {inherit (ix) pkgs;};
      inherit lib target;
      inherit (ix) pkgs writeBashApplication;
    };
    crossGhc = ix.crossGhc {
      inherit lib target toolchain;
      inherit (ix.pkgs) autoconf automake haskellPackages libffi lld llvmPackages perl python3 stdenv;
      nixpkgsPath = ix.pkgs.path;
    };
    crossHaskell = ix.crossHaskell {
      inherit crossGhc lib;
      inherit (ix.pkgs) haskellPackages llvmPackages stdenv;
      writeBashApplication = ix.writeBashApplication ix.pkgs;
    };
    # Same source/version/dependency metadata nixpkgs' by-name package uses,
    # with the forked nix-derivation source swapped in (same override as the
    # native lane, so the two lanes cannot drift).
    rawNom = (pkgs.haskellPackages.extend (_hfinal: hprev: {
      nix-derivation = overrideNixDerivation hprev;
    })).callPackage (pkgs.path + "/pkgs/by-name/ni/nix-output-monitor/generated-package.nix") {};
  in
    crossHaskell.build {
      root = rawNom;
      # streamly-core's own cabal ghc-options say -O2, tuned for
      # fusion-critical streaming pipelines; that -O2 tail (SpecConstr on a
      # few giant modules) runs ~2h single-threaded on the CI runners and
      # dominates the whole lane's build time. nom only walks log lines
      # through it, so cap it at -O1 (Cabal appends configure-time
      # --ghc-option after the package's own flags; GHC honours the last -O).
      configureFlagsFor.streamly-core = ["--ghc-option=-O1"];
      extraNativeBuildInputs = [pkgs.installShellFiles];
      # Mirror the by-name package's postInstall (symlinked aliases plus shell
      # completions); the completions are architecture-independent text.
      postInstall = ''
        # shell
        ln -s nom "$out/bin/nom-build"
        ln -s nom "$out/bin/nom-shell"
        installShellCompletion completions/*
      '';
    };
in
  if ix != null && (ix.cross.isCross or false)
  then crossNom
  else nativeNom
