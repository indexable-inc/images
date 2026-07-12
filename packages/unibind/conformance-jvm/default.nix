{ix}:
# The conformance crate ships nothing: the cdylib comes from the shared
# cargo-unit workspace graph, and the only artifact worth building is the
# proof that the generated Java class behaves. This derivation *is* that
# proof: take the jvm package assembled by `unibind.lib.build` (the
# generated class, the native library under `native/`, the JUnit-less Java
# suite in ./java), compile it warnings-as-errors, and run the suite
# against the real native library. Exposed as `passthru.tests.run`, it
# joins the CI check set as `checks.<system>.unibind-conformance-jvm-run`.
let
  inherit (ix) pkgs;

  built = ix.unibind.build {
    crate = "unibind-conformance-jvm";
    targets.jvm = {
      javaSource = ./java;
    };
  };

  # The generated class uses the final FFM API (JDK 22+); 25 matches the
  # repo's JVM default (lib/languages/jvm-defaults.nix) and the Minestom
  # images.
  jdk = ix.languages.java.jdk pkgs {
    version = "25";
    distribution = "openjdk";
  };

  run = pkgs.stdenv.mkDerivation {
    pname = "unibind-conformance-jvm-run";
    version = "0.1.0";
    src = built.jvm.jvmPackage;
    strictDeps = true;
    nativeBuildInputs = [jdk];

    buildPhase = ''
      # shell
      runHook preBuild
      # Warnings-as-errors, with the two lints the generated class cannot
      # satisfy silenced by category rather than by annotation, so the
      # generated source stays clean: [serial] wants serialVersionUID on
      # exception classes that are never serialized, and [restricted]
      # flags the FFM calls the class exists to make — the real gate is
      # the --enable-native-access runtime flag below.
      javac -Xlint:all,-serial,-restricted -Werror -d classes \
        $(find java -name '*.java' -print)
      runHook postBuild
    '';

    doCheck = true;
    checkPhase = ''
      # shell
      runHook preCheck
      # The suite prints every check (the CI log is the conformance
      # evidence) and exits nonzero on any failure.
      java --enable-native-access=ALL-UNNAMED \
        -Dunibind.library.${built.jvm.libraryKey}="$PWD/native/${built.jvm.soname}" \
        -cp classes ConformanceSuite
      runHook postCheck
    '';

    installPhase = ''
      # shell
      runHook preInstall
      mkdir -p "$out"
      runHook postInstall
    '';

    meta.description = "unibind conformance suite over the generated Java class (javac -Werror + JUnit-less runner)";
  };
in
  run.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests =
          (old.passthru.tests or {})
          // {
            inherit run;
          };
      };
  })
