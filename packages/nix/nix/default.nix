{
  ix,
  lib,
  updateScriptWriter ? null,
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
# new `nix store builds [--json]` command. Patches 0025-0029 carry the lazy
# trees backport (NixOS/nix#15711 plus its post-merge fixes) behind an
# off-by-default `lazy-trees` eval setting: flake inputs mount at their
# content-addressed store paths and only materialize when forced
# (indexable-inc/index#3645). The fork's
# `codex/flake-check-eval-cache` branch (draft PR indexable-inc/nix#1) is
# deliberately excluded: it is self-declared WIP, untested, and incomplete.
let
  # Read `pkgs` from `ix` rather than a `pkgs` callPackage formal: a `src`/`pkgs`
  # formal is fragile against `callPackage` auto-binding, and the rest of the
  # nix/* packages read `pkgs` off their argument the same way.
  inherit (ix) pkgs;

  # Cross lane (RFC 0009, #3585): when the registry cross lane instantiates
  # this package (`cross = true` in package.nix), swap the component scope to
  # the Linux -> Darwin nixpkgs cross scope so the whole modular C++ closure
  # builds on the linux fleet and a Mac only substitutes the fork daemon.
  # Everything modular below reads `componentPkgs`; `pkgs` stays the native
  # package set for build-platform helpers (provenance JSON, update script,
  # test tooling). See lib/darwin/nixpkgs-cross.nix for the scope.
  isCross = ix.cross.isCross or false;
  componentPkgs =
    if isCross
    then ix.cross.pkgs
    else pkgs;
  # `file -bL` prints the architecture in Mach-O spelling.
  machoArch =
    if lib.hasPrefix "aarch64-" (ix.cross.target or "")
    then "arm64"
    else "x86_64";

  bootstrapLockPath = ix.paths.root + "/.github/actions/bootstrap-patched-nix/lock.json";
  bootstrapLock = lib.importJSON bootstrapLockPath;
  updateScriptArgs = {
    name = "nix-ix-bootstrap-lock-update";
    runtimeInputs = [
      pkgs.coreutils
      pkgs.git
    ];
    meta.description = "Resolve the Nix bootstrap source ref into its generated lock";
    text = ''
      # nu
      const lock_path = ".github/actions/bootstrap-patched-nix/lock.json"

      def main [source_ref?: string] {
        let current = (open $lock_path)
        let repository = ($current.repository | into string)
        let requested = ($source_ref | default $current.revision)
        let source_repo = (^mktemp -d | str trim)
        let initialized = (^git init -q $source_repo | complete)
        if $initialized.exit_code != 0 {
          ^rm -rf $source_repo
          error make {msg: $"failed to initialize bootstrap source: ($initialized.stderr | str trim)"}
        }
        let fetched = (
          ^git -C $source_repo fetch --depth 1 $"https://github.com/($repository).git" $requested
          | complete
        )
        if $fetched.exit_code != 0 {
          ^rm -rf $source_repo
          error make {msg: $"failed to fetch bootstrap source `($requested)`: ($fetched.stderr | str trim)"}
        }
        let resolved = (^git -C $source_repo rev-parse FETCH_HEAD | complete)
        ^rm -rf $source_repo
        if $resolved.exit_code != 0 {
          error make {msg: $"failed to resolve bootstrap source `($requested)`: ($resolved.stderr | str trim)"}
        }
        let revision = ($resolved.stdout | str trim)
        {repository: $repository, revision: $revision}
        | to json --indent 2
        | save --force $lock_path
        print $"updated ($lock_path) to ($revision)"
      }
    '';
  };

  # nixpkgs builds `nixVersions.nix_2_34` as
  # `(nixComponents_2_34.overrideSource fetchedSrc).appendPatches patches_common`
  # then takes `.nix-everything` (pkgs/tools/package-management/nix/default.nix).
  # The pinned rev IS the tag 2.34.7 nixpkgs itself fetches (byte-identical
  # narHash), so swapping the fetched source for our patched tree via the same
  # modular `overrideSource` handle rebuilds every component from the patched
  # tree while keeping nixpkgs' interdependency scope and build wiring intact.
  base = componentPkgs.nixVersions.nixComponents_2_34;
  upstreamVersion = lib.removeSuffix "\n" (builtins.readFile (ix.nixSrc + "/.version"));

  # nixpkgs' own whole-source patches for this version: currently just the
  # aarch64-darwin flaky-test skip (empty on every other system). `overrideSource`
  # resets the scope's `patches` to `[]`, so re-apply them here to match a stock
  # `nix_2_34` build; our own delta rides in `patchedSrc`, not here. Gated on
  # existence because the patch lives in the consumer's nixpkgs tree, not ours:
  # nixpkgs 26.11pre dropped it, and a flake that instantiates this package
  # with such a nixpkgs must still evaluate (we already skip test suites on
  # darwin, so losing the flaky-test skip changes nothing we run).
  flakySkipPatch = componentPkgs.path + "/pkgs/tools/package-management/nix/patches/skip-flaky-darwin-tests.patch";
  patchesCommon =
    lib.optional
    (componentPkgs.stdenv.hostPlatform.isDarwin && builtins.pathExists flakySkipPatch)
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
      passthru =
        (old.passthru or {})
        // {
          # The patched modular component set, for tools that must link the
          # same patched libexpr this daemon-compatible client uses
          # (packages/nix/nix-eval-jobs: the CI evaluator has to parse the
          # same language the client does, underscore digit separators
          # included).
          components = patchedComponents;
          inherit provenance;
        };
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
      # The cross build cannot execute its result (the `smoke` passthru is
      # native-only below), so assert the container format in-build: a
      # mislinked binary (ELF, wrong arch) fails on the linux builder instead
      # of on the first Mac that substitutes it.
      nativeBuildInputs = (old.nativeBuildInputs or []) ++ lib.optional isCross pkgs.file;
      installPhase =
        (old.installPhase or "")
        + ''
          install -Dm444 ${provenanceJson} "$out/share/nix/ix-provenance.json"
        ''
        + lib.optionalString isCross ''
          format=$(file -bL "$out/bin/nix")
          # file(1) orders arch and kind differently across versions
          # ("Mach-O 64-bit arm64 executable" vs "... executable arm64").
          case $format in
          "Mach-O 64-bit ${machoArch} executable"* | "Mach-O 64-bit executable ${machoArch}"*) ;;
          *)
            echo "expected a Mach-O 64-bit ${machoArch} executable, got: $format" >&2
            exit 1
            ;;
          esac
        '';
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

  focusedFunctionalTest = {
    name,
    testDaemon ? null,
  }: let
    tests = package.components.nix-functional-tests.override (
      lib.optionalAttrs (testDaemon != null) {test-daemon = testDaemon;}
    );
  in
    tests.overrideAttrs (old: {
      mesonCheckFlags = (old.mesonCheckFlags or []) ++ [name];
    });

  autoGcInterrupt = focusedFunctionalTest {name = "gc-auto";};
  daemonSignal = focusedFunctionalTest {
    name = "daemon-signal";
    testDaemon = package.components.nix-cli;
  };
  buildStatus = focusedFunctionalTest {name = "build-status";};
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        inherit bootstrapLock closureGates;
        # Execution tests are native-only: the cross package's binary cannot
        # run on the linux build host (its format is asserted in-build
        # instead), and the check catalog collects tests from the native
        # `repoPackages` entry only.
        tests =
          (old.passthru.tests or old.tests or {})
          // lib.optionalAttrs (!isCross) {
            inherit autoGcInterrupt buildStatus daemonSignal smoke;
          };
      }
      // lib.optionalAttrs (updateScriptWriter != null) {
        updateScript = updateScriptWriter updateScriptArgs;
      };
  })
