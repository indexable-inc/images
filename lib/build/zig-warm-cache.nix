_:
/**
zig-warm-cache: seed zig's content-addressed compile cache from a base build.

The zig analog of cargo-unit / kbuild-unit's small-delta-small-rebuild
property, at the granularity zig's build model exposes. Zig has no stable
per-translation-unit boundary a Nix decomposition could target -- one
`zig build` invocation compiles the whole module graph -- but its local
cache (`--cache-dir`, the Manifest system) keys entries on file content
digests, not absolute paths. A full build of the UNPATCHED base source
therefore produces a cache a patched build can seed from, recompiling only
the modules an edit actually touches, across differing store paths.

Warming is best-effort: if the base build fails partway (an unpatched base
that only builds WITH the fork series applied, see the installPhase note),
the cache still holds every module compiled before the failure and the
derivation succeeds loudly-with-a-warning rather than failing the graph.

Measured on libghostty-vt (aarch64-darwin, index#3768): cold 2m01s; warm-
seeded rebuild 1m03s, with or without a patch delta (the residual is
linking, install, and the cache copy, not recompilation).

The warm derivation is keyed on `baseSource` alone, so every patch-series
iteration reuses one realized warm cache; it rebuilds only when the
upstream pin moves. Callers whose source never diverges from the base
should skip warming entirely -- it buys nothing.

Arguments (`mkWarmCache`):
- `pname`, `version`: identity of the consuming package; the warm derivation
  is named `<pname>-warm-cache`.
- `baseSource`: the unpatched source tree the cache is warmed from.
- `setup`: shell fragment run before the build (global-cache seeding, SDK
  env). Runs in both the warm build and, via the consumer, the real one.
- `zigArgs`: shell fragment of `zig build` arguments shared with the real
  build. The warm build installs to a throwaway prefix.
- `nativeBuildInputs`, `buildInputs`: toolchain and libraries, matching the
  real build so cache keys agree.

`seedFrom warmCache`: shell fragment for the REAL build that copies the warm
cache into `$TMPDIR/zig-local-cache` (the conventional `--cache-dir`) and
makes it writable. Pass the `mkWarmCache` result, or null for a no-op, so
call sites stay unconditional.
*/
pkgs: {
  mkWarmCache = {
    pname,
    version,
    baseSource,
    setup,
    zigArgs,
    nativeBuildInputs,
    buildInputs,
  }:
    pkgs.stdenv.mkDerivation {
      pname = "${pname}-warm-cache";
      inherit version;

      src = builtins.path {
        name = "${pname}-base-source";
        path = baseSource;
      };

      strictDeps = true;
      inherit nativeBuildInputs buildInputs;

      dontConfigure = true;
      dontBuild = true;

      installPhase = ''
        # shell
        runHook preInstall

        ${setup}
        mkdir -p "$TMPDIR/zig-local-cache"

        # Best-effort: a failed base build still seeds the cache with every
        # module it compiled before failing, and compile artifacts are what
        # the cache is for. Concretely, ghostty pins >= 49a43bf5 cannot
        # complete `-Demit-lib-vt` on darwin UNPATCHED (the vt static-lib
        # archive combine shells to hardcoded /usr/bin/ranlib; the fork
        # series carries the fix), but every Zig module compiles first, so
        # the warm hit rate is unaffected. The real, patched build is the
        # correctness gate and stays strict.
        if ! zig build \
          --cache-dir "$TMPDIR/zig-local-cache" \
          ${zigArgs} \
          --prefix "$TMPDIR/warm-install" \
          --summary all; then
          echo "zig-warm-cache: base build FAILED; seeding from the partial cache." >&2
          echo "zig-warm-cache: the real (patched) build is the correctness gate." >&2
        fi

        cp -R "$TMPDIR/zig-local-cache" "$out"

        runHook postInstall
      '';

      doCheck = false;

      meta.description = "Warm zig compile cache for ${pname}, keyed on the unpatched base source";
    };

  seedFrom = warmCache:
    if warmCache == null
    then ""
    else ''
      cp -R --no-preserve=mode ${warmCache}/. "$TMPDIR/zig-local-cache/"
      chmod -R u+w "$TMPDIR/zig-local-cache"
    '';
}
