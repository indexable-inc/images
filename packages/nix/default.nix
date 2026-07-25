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
# (indexable-inc/index#3645). Patches 0038-0039 import `builtins.wasm`
# from the open upstream PR NixOS/nix#15380 (experimental feature
# `wasm-builtin`, indexable-inc/index#3997) and force deterministic Wasm
# execution (NaN canonicalization, deterministic relaxed SIMD) so eval
# results stay bit-identical across the mixed darwin/linux fleet. The
# `libfetchers: resolve git refs lazily...` patch makes rev-pinned
# `builtins.fetchGit` inputs evaluate without network once cached and keeps
# the git HEAD cache fresh (indexable-inc/index#4028). The fork's
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

  # curl 8.21.0 started consuming the public curl_multi_wakeup() eventfd from
  # inside curl_multi_perform(). That loses a wakeup for callers which perform
  # before polling, and libstore's file-transfer worker is exactly such a
  # caller: it can then sleep for its full 10-second idle timeout. Upstream
  # fixed it by giving the threaded resolver a separate internal wakeup pair
  # (https://github.com/curl/curl/issues/22272).
  #
  # This patch belongs here rather than on `pkgs.curl`, even though the overlay
  # is the usual home for a nixpkgs fix. curl reaches GHC, rustc, cargo and the
  # whole python package set as a build input via git-minimal
  # (ghc -> sphinx -> pytest-xdist -> execnet -> hatch-vcs -> git-minimal ->
  # curl), so overriding it globally rehashes most of nixpkgs and detaches the
  # tree from cache.nixos.org. Measured on 2026-07-25 against nixpkgs
  # e2587cae: arrow-cpp substitutes as a 28.5 MiB download and souffle as
  # 2.8 MiB, and both were being compiled from source here for this one patch.
  # `modular/src/libstore/package.nix` is the only nix component that takes
  # curl as an input, so scoping it to that component keeps the fix where the
  # stall happens and leaves everything else matching the binary cache.
  #
  # Drop this once nixpkgs ships a curl containing
  # 009fd378e8f01c97ebe67a14a41a06d56430f3df. The version assertion makes a
  # nixpkgs curl bump fail visibly instead of silently carrying a stale patch.
  curlWithMultiWakeupFix = assert lib.assertMsg (componentPkgs.curl.version == "8.21.0")
  "remove the curl wakeup patch: expected nixpkgs curl 8.21.0, got ${componentPkgs.curl.version}";
    componentPkgs.curl.overrideAttrs (old: {
      patches =
        (old.patches or [])
        ++ [
          (componentPkgs.fetchurl {
            name = "curl-8.21.0-fix-multi-wakeup.patch";
            url = "https://github.com/curl/curl/commit/009fd378e8f01c97ebe67a14a41a06d56430f3df.patch";
            hash = "sha256-RMFcifj9jDaWY5jNBGqQc2NUoXb3+mHR/1ubrYjpHvc=";
          })
        ];
    });

  # The source is the indexable-inc/nix jj megamerge (nix-src input): the
  # upstream 2.34.7 base plus the patch DAG, fetched already patched, so the
  # only remaining build-time patches are nixpkgs' own (`patchesCommon`).
  #
  # Identify a patched daemon by version: `nix --version` (and
  # `builtins.nixVersion`) report the version each *component* was compiled
  # with -- the modular build's preConfigure writes the component derivation's
  # `version` into the tree's `.version` on every build, so a `.version` source
  # patch in our series would be clobbered and a version override on the
  # `nix-everything` aggregate would only rename the store path. Set it through
  # `overrideAllMesonComponents`, the last layer in the component builder
  # stack. The marker is semver build metadata (`+ix.g<rev12>.h<hash>`), not
  # `-ix`: meson feeds the version to darwin ld's -current_version, which
  # rejects a `-` suffix as a "malformed 32-bit x.y.z version number" but
  # tolerates `+`.
  patchedNix = let
    # nixpkgs' modular components.nix derives each component's sourceRoot
    # from `patchedSrc.name`; a raw flake input (fetchTree result) carries no
    # `name`. stdenv unpacks a store-path src into `stripHash $src`, which is
    # "source" for a github tarball input, so declare exactly that.
    patchedSrc = ix.nixSrc // {name = "source";};
    source =
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
      sourceNarHash = source.narHash;
      inherit commonPatches;
    });
    shortHash = builtins.substring 0 20 sourceDigest;
    revStamp =
      if ix.nixSrc ? rev
      then "g${builtins.substring 0 12 ix.nixSrc.rev}."
      else "";
    version = "${upstreamVersion}+ix.${revStamp}h${builtins.substring 0 8 sourceDigest}";
    provenance = {
      schema = 2;
      algorithm = "sha256";
      inherit commonPatches sourceDigest source version;
    };
    provenanceJson = (pkgs.formats.json {}).generate "nix-ix-provenance.json" provenance;
    patchedComponents = (((base.overrideSource patchedSrc).appendPatches patchesCommon).overrideAllMesonComponents
      (_: _: {inherit version;}))
      .overrideScope (_: prev: {
      # Patch 0038 (builtins.wasm, NixOS/nix#15380) adds a `wasm` meson
      # feature to libexpr and declares its wasmtime dependency in the
      # in-tree src/libexpr/package.nix; the nixpkgs modular scope builds
      # from its own vendored component packaging, so that declaration
      # never reaches this build. Worse, nixpkgs' meson hook passes
      # --auto-features=enabled, which flips the feature on with nobody
      # supplying the library (wasm.cc fails on #include <wasmtime.hh>).
      # Wire the dependency here, at the same seam that swaps the source,
      # and pin the feature explicitly so a missing wasmtime fails at
      # configure time instead of mid-compile. nixpkgs' wasmtime ships the
      # C API + C++ headers in its dev output and libwasmtime via the
      # dev->out propagation of the multiple-outputs hook.
      nix-expr = prev.nix-expr.overrideAttrs (old: {
        buildInputs = (old.buildInputs or []) ++ [componentPkgs.wasmtime];
        mesonFlags = (old.mesonFlags or []) ++ ["-Dwasm=enabled"];
      });
      # See `curlWithMultiWakeupFix` above: libstore owns the file-transfer
      # worker the curl regression stalls, and it is the only component that
      # takes curl, so the patch is scoped to it instead of `pkgs.curl`.
      nix-store = prev.nix-store.override {curl = curlWithMultiWakeupFix;};
    });

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
          # (packages/nix-eval-jobs: the CI evaluator has to parse the
          # same language the client does, underscore digit separators
          # included).
          components = patchedComponents;
          inherit provenance;
        };
      # The aggregate's `doCheck = true` gates the build on `checkInputs`: the
      # five component unit-test runners plus the entire upstream functional
      # suite. Those dominate a cold build of this closure and re-validate
      # nothing per consumer rebuild: the source arrives pre-patched from the
      # fork repo, the series carries its own
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
          description = "NixOS/nix ${upstreamVersion} with the index patch DAG (indexable-inc/nix megamerge, h${shortHash})";
          mainProgram = "nix";
        };
    });

  package = patchedNix;

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
        --arg upstreamNarHash ${lib.escapeShellArg package.provenance.source.narHash} \
        '.schema == 2 and .algorithm == "sha256" and .version == $version and .sourceDigest == $sourceDigest and .source.narHash == $upstreamNarHash' \
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
  # `libfetchers: resolve git refs lazily and refresh the cached HEAD`
  # regression coverage: a cached rev-pinned fetchGit input must evaluate
  # without remote git subprocesses, and the cached HEAD must refresh on a
  # successful network lookup (indexable-inc/index#4028).
  fetchGitHeadCache = focusedFunctionalTest {name = "fetchGit-head-cache";};
  daemonSignal = focusedFunctionalTest {
    name = "daemon-signal";
    testDaemon = package.components.nix-cli;
  };
  buildStatus = focusedFunctionalTest {name = "build-status";};
  # Patch 0024 regression coverage: the upstream relative-paths lock file
  # test now asserts sparse child-lock semantics (stale copied nodes refresh
  # from the child's own flake.lock; in-sync locks stay byte-identical).
  sparseLocks = focusedFunctionalTest {name = "relative-paths-lockfile";};
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        inherit bootstrapLock;
        # Execution tests are native-only: the cross package's binary cannot
        # run on the linux build host (its format is asserted in-build
        # instead), and the check catalog collects tests from the native
        # `repoPackages` entry only.
        tests =
          (old.passthru.tests or old.tests or {})
          // lib.optionalAttrs (!isCross) {
            inherit autoGcInterrupt buildStatus daemonSignal fetchGitHeadCache smoke sparseLocks;
          };
      }
      // lib.optionalAttrs (updateScriptWriter != null) {
        updateScript = updateScriptWriter updateScriptArgs;
      };
  })
