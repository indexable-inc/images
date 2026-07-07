{
  ix,
  pkgs ? ix.pkgs,
}:
# The conformance crate ships nothing: the cdylib comes from the shared
# cargo-unit workspace graph with the unibind `jvm` feature on, and the only
# artifact worth building is the proof that the generated JVM surface
# behaves from both host languages. This derivation *is* that proof: compile
# the unibind-generated Java + Kotlin sources together with the committed
# runners, point them at the built cdylib, and run both (envelope decoding,
# mirror layouts, unicode, defaults and overloads, exception mapping, panic
# containment). Both transcripts land in $out as java.txt / kotlin.txt.
# `passthru.tests` gates it in CI as
# `checks.<system>.unibind-conformance-jvm-run`, next to the crate's own
# unit gates (clippy over the expanded glue included).
let
  inherit
    (ix.unibind.build {
      crate = "unibind-conformance-jvm";
      targets.jvm = {};
    })
    jvm
    ;

  units = ix.cargoUnit.selectLibraryWithTests ix.rustWorkspace.units {
    library = "unibind_conformance_jvm";
    packageName = "unibind-conformance-jvm";
  };

  runner = ./runner;

  # `java.lang.foreign` is final since JDK 22; jdk25 is the lowest JDK past
  # that line in the pinned nixpkgs (22-24 are non-LTS and already dropped).
  jdk = pkgs.jdk25;

  run =
    pkgs.runCommand "unibind-conformance-jvm-run"
    {
      strictDeps = true;
      nativeBuildInputs = [
        jdk
        pkgs.kotlin
      ];
      meta.description = "unibind JVM conformance: the Java and Kotlin Panama runners over the generated bindings";
    }
    ''
      set -o pipefail
      cdylib=""
      for candidate in \
        ${jvm.library}/lib/libunibind_conformance_jvm.so \
        ${jvm.library}/lib/libunibind_conformance_jvm-*.so \
        ${jvm.library}/lib/libunibind_conformance_jvm.dylib \
        ${jvm.library}/lib/libunibind_conformance_jvm-*.dylib
      do
        if [ -f "$candidate" ]; then
          cdylib="$candidate"
          break
        fi
      done
      if [ -z "$cdylib" ]; then
        echo "unibind-conformance-jvm: no cdylib under ${jvm.library}/lib" >&2
        ls -la ${jvm.library}/lib >&2 || true
        exit 1
      fi

      # kotlinc writes compiler caches under $HOME.
      export HOME="$TMPDIR"
      mkdir classes
      javac -d classes ${jvm.sources}/unibind/conformance/*.java ${runner}/Main.java
      kotlinc -classpath classes -d classes \
        ${jvm.sources}/unibind/conformance/Conformance.kt ${runner}/main.kt

      mkdir -p "$out"
      java -cp classes \
        --enable-native-access=ALL-UNNAMED \
        -Dunibind.conformance.library="$cdylib" \
        Main | tee "$out/java.txt"
      java -cp classes:${pkgs.kotlin}/lib/kotlin-stdlib.jar \
        --enable-native-access=ALL-UNNAMED \
        -Dunibind.conformance.library="$cdylib" \
        MainKt | tee "$out/kotlin.txt"
    '';
in
  run.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests =
          (units.passthru.tests or {})
          // {
            inherit run;
          };
      };
  })
