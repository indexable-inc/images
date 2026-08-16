{
  ix,
  # Threaded whole into the toolchain helpers (`ix.languages.java.jdk pkgs`,
  # `ix.languages.kotlin.compiler pkgs`), so there is no fixed dep list to
  # enumerate and nothing for `override` to reach past.
  # astlog-ignore: no-pkgs-in-callpackage
  pkgs ? ix.pkgs,
}: let
  # mc-probe-kt calls `McProtocolJvm`, the unibind-rendered Java class over
  # the Rust mc-protocol crate, so the Python probe, this probe, and the
  # servers' tests speak the wire format through one implementation. The
  # build output pairs the generated class with the native library it
  # dlopens (`jvmPackage`: `java/` + `native/<soname>`).
  built = ix.unibind.build {
    crate = "mc-protocol-jvm";
    targets.jvm = {};
  };

  # The generated class uses the final FFM API (JDK 22+); 25 matches the
  # repo's JVM default (lib/languages/jvm-defaults.nix) and the Minestom
  # images.
  jdk = ix.languages.java.jdk pkgs {
    version = "25";
    distribution = "openjdk";
  };

  kotlinc = ix.languages.kotlin.compiler pkgs {target = "jvm";};
in
  pkgs.stdenv.mkDerivation {
    pname = "mc-probe-kt";
    version = "0.1.0";
    src = ./src;
    strictDeps = true;
    # The tree ships no tests for this small kotlinc build.
    doCheck = false;
    nativeBuildInputs = [
      jdk
      kotlinc
      pkgs.makeWrapper
    ];

    buildPhase = ''
      # shell
      runHook preBuild
      # Warnings-as-errors on the generated Java class, with the two lints
      # it cannot satisfy silenced by category rather than by annotation
      # (same policy as packages/unibind/conformance-jvm): [serial] wants
      # serialVersionUID on exception classes that are never serialized,
      # and [restricted] flags the FFM calls the class exists to make —
      # the real gate is the --enable-native-access runtime flag below.
      javac -Xlint:all,-serial,-restricted -Werror -d classes \
        $(find ${built.jvm.jvmPackage}/java -name '*.java' -print)
      kotlinc -Werror -classpath classes -d classes McProbe.kt
      runHook postBuild
    '';

    installPhase = ''
      # shell
      runHook preInstall
      mkdir -p "$out/share/mc-probe-kt"
      cp -r classes "$out/share/mc-probe-kt/classes"
      # kotlinc is a launcher script; the runtime jar the compiled classes
      # need lives inside its tree.
      stdlib="$(find ${kotlinc} -name 'kotlin-stdlib.jar' -print -quit)"
      if [ -z "$stdlib" ]; then
        echo "mc-probe-kt: kotlin-stdlib.jar not found under ${kotlinc}" >&2
        exit 1
      fi
      makeWrapper ${jdk}/bin/java "$out/bin/mc-probe-kt" \
        --add-flags "--enable-native-access=ALL-UNNAMED" \
        --add-flags "-Dunibind.library.${built.jvm.libraryKey}=${built.jvm.jvmPackage}/native/${built.jvm.soname}" \
        --add-flags "-cp $out/share/mc-probe-kt/classes:$stdlib" \
        --add-flags "McProbeKt"
      runHook postInstall
    '';

    meta = {
      description = "Assert Minecraft Server List Ping responses, from Kotlin over the unibind JVM bindings";
      mainProgram = "mc-probe-kt";
    };
  }
