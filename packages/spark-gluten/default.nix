# Apache Gluten Velox-backend bundle for Spark 3.5, patched to run on NixOS.
#
# Stock Spark executes queries on the JVM. Gluten offloads the physical
# operators to Velox, a vectorized C++ engine, which is where the large
# analytical speedups over plain Spark come from. Upstream ships one fat jar per
# Spark line with the Velox and Arrow JNI native libraries (`libvelox.so`,
# `libgluten.so`, `libarrow_*_jni.so`) packed inside; Gluten extracts them to a
# temp dir and `dlopen()`s them at runtime.
#
# Those libraries are built on CentOS 7, so their ELF interpreter is
# `/lib64/ld-linux-x86-64.so.2` and their rpath points at FHS paths that do not
# exist on NixOS: the stock jar fails to load. We explode the jar, autoPatchelf
# the native libs against the nix store, and repack. The patched interpreter and
# rpath are absolute store paths, so they survive Gluten's runtime
# re-extraction.
#
# Bump: change `version`, refresh `src.hash` with `nix-prefetch-url`, and check
# the tarball against its published `.sha512` on archive.apache.org. Only a
# linux_amd64 native build is published upstream.
{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
  patchelf,
  unzip,
  zip,
}:
let
  version = "1.6.0";
  sparkVersion = "3.5";
  scalaVersion = "2.12";
  jarName = "gluten-velox-bundle.jar";
in
stdenv.mkDerivation (finalAttrs: {
  pname = "spark-gluten";
  inherit version;

  # The archive host keeps every release. The tarball holds two files: a
  # DISCLAIMER and the bundle jar.
  src = fetchurl {
    url = "https://archive.apache.org/dist/gluten/${version}/apache-gluten-${version}-bin-spark-${sparkVersion}.tar.gz";
    hash = "sha256-kPnsaslkvNc4k8ZhqRM+Rq/Eb4svqeF8ZH+zk5uLbUM=";
  };

  dontUnpack = true;
  strictDeps = true;

  nativeBuildInputs = [
    autoPatchelfHook
    patchelf
    unzip
    zip
  ];

  # The CentOS 7 build statically links Velox/Arrow and their third-party deps
  # (vcpkg static), so the only dynamic dependencies of the bundled `.so`s are
  # glibc and the intra-bundle libvelox.so -> libgluten.so sibling link. glibc is
  # provided by the host JVM's already-loaded namespace at runtime; libgcc_s /
  # libstdc++ are kept here as a defensive rpath entry in case a future bundle
  # links them dynamically.
  buildInputs = [
    (lib.getLib stdenv.cc.cc)
  ];

  # libvelox.so is ~246 MiB; stripping a vendored binary is slow and pointless.
  dontStrip = true;

  # We autopatch the libs inside the exploded jar and repack, so there is
  # nothing left in $out for the automatic fixup pass to find.
  dontAutoPatchelf = true;

  buildPhase = ''
    runHook preBuild
    tar xzf "$src"
    mkdir exploded
    ( cd exploded && unzip -q ../*.jar )

    # Validate every native dependency is satisfiable from buildInputs (this
    # errors the build if one is missing) and let autoPatchelf resolve them.
    autoPatchelf exploded

    # autoPatchelf records absolute build-sandbox paths (e.g. /build/exploded)
    # for the one intra-bundle link, libvelox.so -> libgluten.so. Those vanish
    # once Gluten extracts the libraries to a runtime temp dir, exactly like the
    # dangling /workspace path upstream ships. Re-root each library at $ORIGIN so
    # a co-extracted sibling resolves, and append the store paths of the external
    # deps so libstdc++/openssl/numa/zlib resolve wherever the libs land.
    for so in exploded/linux/amd64/*.so exploded/x86_64/*.so; do
      patchelf --set-rpath "\$ORIGIN:${lib.makeLibraryPath finalAttrs.buildInputs}" "$so"
    done

    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    mkdir -p "$out/share/java"
    ( cd exploded && zip -q -r -1 "$out/share/java/${jarName}" . )
    runHook postInstall
  '';

  passthru = {
    inherit sparkVersion scalaVersion;
    # Absolute path consumers put on the Spark driver/executor classpath.
    jar = "${finalAttrs.finalPackage}/share/java/${jarName}";
  };

  meta = {
    description = "Apache Gluten Velox backend bundle for Spark ${sparkVersion}, patched for NixOS";
    homepage = "https://gluten.apache.org/";
    license = lib.licenses.asl20;
    platforms = [ "x86_64-linux" ];
    sourceProvenance = [ lib.sourceTypes.binaryNativeCode ];
  };
})
