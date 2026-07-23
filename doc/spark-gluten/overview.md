# spark-gluten

`packages/spark-gluten` packages the
[Apache Gluten](https://gluten.apache.org/) Velox-backend bundle for Spark 3.5,
patched so its native libraries load on NixOS. Stock Spark runs queries on the
JVM; Gluten offloads the physical operators to Velox, a vectorized C++ engine,
which is where the large analytical speedups come from. Upstream ships one fat
jar per Spark line with the Velox/Arrow JNI native libraries packed inside;
Gluten extracts them to a temp dir and `dlopen()`s them at runtime. This
package is consumed by the spark service module alongside
[spark-hive](../spark-hive/overview.md).

## The NixOS problem and the fix

The bundled `.so`s (`libvelox.so`, `libgluten.so`, `libarrow_*_jni.so`) are
built on CentOS 7, so their ELF interpreter is `/lib64/ld-linux-x86-64.so.2` and
their rpath points at FHS paths that do not exist on NixOS: the stock jar fails
to load (`packages/spark-gluten/default.nix:1-15`). The fix is to explode the
jar, autopatchelf the native libs against the Nix store, and repack; the patched
interpreter and rpath are absolute store paths, so they survive Gluten's runtime
re-extraction.

`buildPhase` (`default.nix:73-94`):

1. `tar xzf` the release tarball, then `unzip` the bundle jar into `exploded/`.
2. `autoPatchelf exploded` validates every native dependency is satisfiable from
   `buildInputs` (it errors the build if one is missing) and rewrites
   interpreters/rpaths.
3. For each `exploded/linux/amd64/*.so` and `exploded/x86_64/*.so`, `patchelf
   --set-rpath "$ORIGIN:<libPath>"`: autoPatchelf records the absolute build-
   sandbox path for the intra-bundle `libvelox.so -> libgluten.so` link, which
   vanishes once Gluten extracts to a runtime temp dir, so re-rooting each
   library at `$ORIGIN` lets a co-extracted sibling resolve, and the appended
   store paths cover external deps.

`installPhase` repacks `exploded/` into
`$out/share/java/gluten-velox-bundle.jar` (`default.nix:96-101`).

Notable build settings:

- `buildInputs = [ (lib.getLib stdenv.cc.cc) ]`: the CentOS 7 build statically
  links Velox/Arrow and their vcpkg deps, so the only dynamic deps are glibc
  (provided by the host JVM's namespace at runtime) and the intra-bundle link;
  libgcc_s/libstdc++ are kept as a defensive rpath entry
  (`default.nix:56-64`).
- `dontStrip = true` (libvelox.so is ~246 MiB; stripping a vendored binary is
  slow and pointless) and `dontAutoPatchelf = true` (patching happens inside the
  exploded jar in `buildPhase`, so the automatic fixup has nothing to find)
  (`default.nix:66-72`).

## Versions and passthru

Pinned inline in `default.nix`: `version = "1.6.0"`, `sparkVersion = "3.5"`,
`scalaVersion = "2.12"` (`default.nix:30-33`); `src` is `fetchurl` of the
`apache-gluten-${version}-bin-spark-${sparkVersion}.tar.gz` from
`archive.apache.org` with an inline SRI `hash` (`default.nix:41-44`).
`passthru` exposes `sparkVersion`, `scalaVersion`, and `jar` (the absolute path
consumers put on the Spark driver/executor classpath, `default.nix:103-107`).

## Build and wiring

- Bump: change `version`, refresh `src.hash` with `nix-prefetch-url`, and check
  the tarball against its published `.sha512` on archive.apache.org
  (`default.nix:17-19`). No `manifest.json`/`updateScript`.
- Flake output: `nix run .#spark-gluten` (rarely run directly; it is a classpath
  jar). `package.nix` gates `packageSet` and `flake` to `x86_64-linux` (only a
  linux_amd64 native build is published upstream and the package autopatchelfs
  ELF objects), while `overlay = true` stays unconditional and lazy:
  `pkgs.spark-gluten` is only forced inside an x86_64-linux closure (the spark
  service module), mirroring `drgn` (`packages/spark-gluten/package.nix:3-13`).
- Platform constraint: `meta.platforms = [ "x86_64-linux" ]`
  (`default.nix:109-115`).
