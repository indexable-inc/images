{
  ix,
  lib,
}:
# Upstream NixOS/nix pinned at tag 2.34.7 (the `nix-src` input, surfaced as
# `ix.nixSrc`) with the in-repo patch series (./patches) applied, built through
# nixpkgs' own modular nix packaging so the result is a protocol-compatible
# drop-in for the daemon the fleet runs.
#
# De-forking replacement for a standalone `indexable-inc/nix` fork checkout:
# instead of tracking a whole fork branch, the delta lives as an ordered
# `patches/` series applied on top of the exact upstream rev the daemon runs.
# The current series carries the GC-roots client-interrupt daemon crash fix
# (extracted from the fork's `fix-gc-roots-client-interrupt-crash` branch,
# complete and clean against 2.34.7), an eval fix treating inaccessible
# default lookup-path entries as absent (found while validating this build:
# the macOS sandbox denies the host's root-channels dir with EPERM, which
# aborted the C API unit tests and the recursive-nix functional test on any
# darwin host with root channels; clean CI builders lack the path, which is
# why caches carry the stock drvs green), and the `build-status-dir` global
# build-observability series (patches 0003..0009): behind an experimental
# feature of that name, every active build/substitution goal writes a JSON
# status file under `<nixStateDir>/status/`, readable daemonlessly via the
# new `nix store builds [--json]` command. The fork's
# `codex/flake-check-eval-cache` branch (draft PR indexable-inc/nix#1) is
# deliberately excluded: it is self-declared WIP, untested, and incomplete.
let
  # Read `pkgs` from `ix` rather than a `pkgs` callPackage formal: a `src`/`pkgs`
  # formal is fragile against `callPackage` auto-binding, and the rest of the
  # nix/* packages read `pkgs` off their argument the same way.
  inherit (ix) pkgs;

  # nixpkgs builds `nixVersions.nix_2_34` as
  # `(nixComponents_2_34.overrideSource fetchedSrc).appendPatches patches_common`
  # then takes `.nix-everything` (pkgs/tools/package-management/nix/default.nix).
  # The pinned rev IS the tag 2.34.7 nixpkgs itself fetches (byte-identical
  # narHash), so swapping the fetched source for our patched tree via the same
  # modular `overrideSource` handle rebuilds every component from the patched
  # tree while keeping nixpkgs' interdependency scope and build wiring intact.
  base = pkgs.nixVersions.nixComponents_2_34;
  upstreamVersion = lib.removeSuffix "\n" (builtins.readFile (ix.nixSrc + "/.version"));

  # nixpkgs' own whole-source patches for this version: currently just the
  # aarch64-darwin flaky-test skip (empty on every other system). `overrideSource`
  # resets the scope's `patches` to `[]`, so re-apply them here to match a stock
  # `nix_2_34` build; our own delta rides in `patchedSrc`, not here. Gated on
  # existence because the patch lives in the consumer's nixpkgs tree, not ours:
  # nixpkgs 26.11pre dropped it, and a flake that instantiates this package
  # with such a nixpkgs must still evaluate (we already skip test suites on
  # darwin, so losing the flaky-test skip changes nothing we run).
  flakySkipPatch = pkgs.path + "/pkgs/tools/package-management/nix/patches/skip-flaky-darwin-tests.patch";
  patchesCommon =
    lib.optional
    (pkgs.stdenv.hostPlatform.isDarwin && builtins.pathExists flakySkipPatch)
    flakySkipPatch;

  # The whole patched pipeline as a function of the applied series, so the
  # per-attempt-patch closure gates below rebuild the SAME logic with a
  # restricted series instead of copying it. `patchNames = null` is the full
  # series (the shipped package).
  #
  # The full-series patched tree doubles as the
  # `checks.<system>.patched-src-nix` conflict gate (per-system wiring), so a
  # patch that stops applying fails there in seconds.
  #
  # Identify a patched daemon by version: `nix --version` (and
  # `builtins.nixVersion`) report the version each *component* was compiled
  # with -- the modular build's preConfigure writes the component derivation's
  # `version` into the tree's `.version` on every build, so a `.version` source
  # patch in our series would be clobbered and a version override on the
  # `nix-everything` aggregate would only rename the store path. Set it through
  # `overrideAllMesonComponents`, the last layer in the component builder
  # stack, which also wins over the sourceLayer's `+<patch-count>` suffix.
  # The marker is semver build metadata (`+ix.p<count>.h<hash>`), not
  # `-ix`: meson feeds the version to darwin ld's -current_version, which
  # rejects a `-` suffix as a "malformed 32-bit x.y.z version number" but
  # tolerates `+`.
  mkPatchedNix = patchNames: let
    patchedSrc = ix.patchedSrc {
      name = "nix";
      src = ix.nixSrc;
      patchDir = ./patches;
      inherit patchNames;
    };
    upstream =
      {
        version = upstreamVersion;
        narHash = ix.nixSrc.narHash;
      }
      // lib.optionalAttrs (ix.nixSrc ? rev) {
        revision = ix.nixSrc.rev;
      };
    commonPatches =
      map (path: {
        name = baseNameOf path;
        digest = builtins.hashString "sha256" (builtins.readFile path);
      })
      patchesCommon;
    sourceDigest = builtins.hashString "sha256" (builtins.toJSON {
      upstreamNarHash = upstream.narHash;
      patchSetDigest = patchedSrc.patchSet.digest;
      inherit commonPatches;
    });
    shortHash = builtins.substring 0 20 sourceDigest;
    version = "${upstreamVersion}+ix.p${toString patchedSrc.patchSet.count}.h${shortHash}";
    provenance = {
      schema = 1;
      algorithm = "sha256";
      inherit commonPatches sourceDigest upstream version;
      inherit (patchedSrc) patchSet;
    };
    provenanceJson = (pkgs.formats.json {}).generate "nix-ix-provenance.json" provenance;
    patchedComponents =
      ((base.overrideSource patchedSrc).appendPatches patchesCommon).overrideAllMesonComponents
      (_: _: {inherit version;});

    # The aggregate `nix` package (daemon + client + libs), the same attribute
    # `nixVersions.nix_2_34` exposes.
    nixEverything = patchedComponents.nix-everything;
  in
    nixEverything.overrideAttrs (old: {
      inherit version;
      # The aggregate's `doCheck = true` gates the build on `checkInputs`: the
      # five component unit-test runners plus the entire upstream functional
      # suite. Those dominate a cold build of this closure and re-validate
      # nothing per consumer rebuild: patch applicability is already gated by
      # `checks.<system>.patched-src-nix`, the series carries its own
      # upstream-style functional test inside the patched tree, and the `smoke`
      # passthru below executes the linked binary. With them on, the cache-push
      # darwin lane (3-core hosted mac) blew its 4 h job budget cold-building
      # this package and froze `cache-ready` (run 28772327218, index#1967).
      doCheck = false;
      installPhase =
        (old.installPhase or "")
        + ''
          install -Dm444 ${provenanceJson} "$out/share/nix/ix-provenance.json"
        '';
      passthru =
        (old.passthru or {})
        // {
          inherit provenance;
        };
      meta =
        (old.meta or {})
        // {
          description = "NixOS/nix ${upstreamVersion} with the index in-repo patch series (${toString provenance.patchSet.count} patches, h${shortHash})";
          mainProgram = "nix";
        };
    });

  package = mkPatchedNix null;

  # Per-attempt-patch closure build gates (RFC 0010 A3, #2098): one derivation
  # per attempt-marked patch, this same package rebuilt with the series
  # restricted to that patch's dag.json closure -- exactly the standalone
  # series `upstream-pr` ships upstream. Lazy passthru data, never a flake
  # check (heavy builds; the scheduled fork-closure-gates workflow and the
  # `upstream-sync --open` preflight build them). Keyed here off the fork's
  # own lib/fork-packages.nix entry so intent has one home.
  closureGates = ix.forkClosureGates.mkGates {
    fork =
      lib.findFirst (fork: fork.name == "nix")
      (throw "packages/nix/nix: lib/fork-packages.nix has no `nix` entry")
      ix.forkPackages;
    patchDir = ./patches;
    mkSeries = mkPatchedNix;
  };

  # The override's real risk is that the whole modular C++ tree still links and
  # the installed binary and provenance file agree with the eval-time identity.
  # `--version` exits without touching a store or daemon, so it is safe here.
  smoke =
    pkgs.runCommand "nix-ix-smoke"
    {
      nativeBuildInputs = [
        package
        pkgs.jq
      ];
      strictDeps = true;
    }
    ''
      expected=${lib.escapeShellArg "nix (Nix) ${package.version}"}
      actual=$(nix --version)
      if [[ "$actual" != "$expected" ]]; then
        echo "nix --version disagrees with the package version" >&2
        printf 'expected: %s\nactual:   %s\n' "$expected" "$actual" >&2
        exit 1
      fi

      jq -e \
        --arg version ${lib.escapeShellArg package.version} \
        --arg sourceDigest ${lib.escapeShellArg package.provenance.sourceDigest} \
        --arg upstreamNarHash ${lib.escapeShellArg package.provenance.upstream.narHash} \
        --arg patchSetDigest ${lib.escapeShellArg package.provenance.patchSet.digest} \
        '.schema == 1 and .algorithm == "sha256" and .version == $version and .sourceDigest == $sourceDigest and .upstream.narHash == $upstreamNarHash and .patchSet.algorithm == "sha256" and .patchSet.digest == $patchSetDigest and .patchSet.count == (.patchSet.patches | length)' \
        ${package}/share/nix/ix-provenance.json >/dev/null

      mkdir -p "$out"
    '';
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        inherit closureGates;
        tests =
          (old.passthru.tests or old.tests or {})
          // {
            inherit smoke;
          };
      };
  })
