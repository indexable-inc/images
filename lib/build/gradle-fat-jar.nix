/**
  Build a Gradle fat-jar with a pinned dependency-verification metadata file.

  Wraps `pkgs.stdenv.mkDerivation` with Gradle as the build tool. The
  dependency hashes come from a Gradle dependency-verification XML
  reproduced into the build sandbox, so the build is fixed-output and
  network-isolated. The selected Gradle task runs in offline mode.

  Arguments:
  - `pname`, `version`, `src`: derivation identity and source.
  - `verificationMetadata`: path to the Gradle verification XML.
  - `javaPackage`, `gradle`: toolchain packages.
  - `gradleBuildTask`, `gradleCheckTask`, `gradleFlags`: build invocation.
  - `jarPath`: relative path of the produced jar inside the source tree.
  - `installPhase`: override the default `cp ${jarPath} $out/...` phase.
  - `doCheck`: run `gradleCheckTask` before the build task.
  - Other standard `mkDerivation` args (`nativeBuildInputs`, `meta`,
    `passthru`, `preConfigure`, etc.) are forwarded.
*/
{ lib }:

pkgs:

{
  pname,
  version,
  src,
  verificationMetadata,
  javaPackage ? pkgs.jdk25,
  gradle ? pkgs.gradle_9,
  gradleBuildTask ? "jar",
  gradleCheckTask ? "check",
  gradleFlags ? [ ],
  jarPath ? "build/libs/${pname}-${version}.jar",
  nativeBuildInputs ? [ ],
  doCheck ? false,
  installPhase ? null,
  meta ? { },
  passthru ? { },
  preConfigure ? "",
  ...
}@args:
let
  extraArgs = builtins.removeAttrs args [
    "pname"
    "version"
    "src"
    "verificationMetadata"
    "javaPackage"
    "gradle"
    "gradleBuildTask"
    "gradleCheckTask"
    "gradleFlags"
    "jarPath"
    "nativeBuildInputs"
    "doCheck"
    "installPhase"
    "meta"
    "passthru"
    "preConfigure"
  ];

  lines = lib.splitString "\n" (builtins.readFile verificationMetadata);

  attrFromLine =
    attr: line:
    let
      match = builtins.match ''.* ${attr}="([^"]+)".*'' line;
    in
    if match == null then null else builtins.head match;

  # Gradle records each artifact digest as a hex `value="<hex>"`. Convert it to
  # the self-describing SRI form the fetcher `hash` slot expects, rather than
  # carrying a legacy `sha256` attr.
  hexToSri =
    hex:
    builtins.convertHash {
      hash = hex;
      hashAlgo = "sha256";
      toHashFormat = "sri";
    };

  # Walk the verification XML line by line, carrying the open component and
  # artifact in the fold accumulator. Gradle emits one tag per line:
  #   <component group="…" name="…" version="…">   opens a component
  #   <artifact name="…">                          opens an artifact in it
  #   <sha256 value="<hex>"/>                       the artifact's sha256 digest
  #   </artifact> / </component>                    close and reset state
  # Only the first `<sha256>` of an artifact is taken; any other digest or
  # signature line (sha512, sha1, md5, pgp, also-trust) is ignored, so a 128-hex
  # or 40-hex fingerprint never reaches `hexToSri`. Close tags clear the open
  # artifact/component so a stray digest line is never reattributed.
  collect =
    state: line:
    if lib.hasInfix "<component " line then
      state
      // {
        component = {
          group = attrFromLine "group" line;
          name = attrFromLine "name" line;
          version = attrFromLine "version" line;
        };
        artifact = null;
      }
    else if lib.hasInfix "</component>" line then
      state
      // {
        component = null;
        artifact = null;
      }
    else if lib.hasInfix ''<artifact name="'' line then
      state // { artifact = attrFromLine "name" line; }
    else if lib.hasInfix "</artifact>" line then
      state // { artifact = null; }
    else if
      lib.hasInfix ''<sha256 value="'' line && state.component != null && state.artifact != null
    then
      state
      // {
        # First digest wins: clear the artifact so a later digest line under the
        # same artifact cannot emit a second record for the same file.
        artifact = null;
        results = state.results ++ [
          (
            state.component
            // {
              file = state.artifact;
              hash = hexToSri (attrFromLine "value" line);
            }
          )
        ];
      }
    else
      state;

  artifacts =
    (lib.foldl' collect {
      component = null;
      artifact = null;
      results = [ ];
    } lines).results;

  artifactUrl =
    {
      group,
      name,
      version,
      file,
      ...
    }:
    "https://repo.maven.apache.org/maven2/${
      lib.replaceStrings [ "." ] [ "/" ] group
    }/${name}/${version}/${file}";

  fetchedArtifacts = map (
    artifact:
    artifact
    // {
      src = pkgs.fetchurl {
        url = artifactUrl artifact;
        inherit (artifact) hash;
      };
    }
  ) artifacts;

  mavenRepo = pkgs.runCommand "${pname}-maven-repository" { } (
    ''
      runHook preInstall
    ''
    + lib.concatMapStringsSep "\n" (
      artifact:
      let
        path = "${
          lib.replaceStrings [ "." ] [ "/" ] artifact.group
        }/${artifact.name}/${artifact.version}/${artifact.file}";
      in
      ''
        mkdir -p "$out/${dirOf path}"
        ln -s ${artifact.src} "$out/${path}"
      ''
    ) fetchedArtifacts
    + ''

      runHook postInstall
    ''
  );

  localMavenInitScript = pkgs.writeText "gradle-local-maven-repository.init.gradle" ''
    gradle.projectsLoaded {
      rootProject.allprojects {
        buildscript.repositories.clear()
        buildscript.repositories.maven {
          url = uri("file://${mavenRepo}")
        }
      }
    }
  '';
in
pkgs.stdenvNoCC.mkDerivation (
  _:
  extraArgs
  // {
    inherit
      pname
      version
      src
      doCheck
      gradleBuildTask
      gradleCheckTask
      passthru
      ;

    strictDeps = true;
    nativeBuildInputs = [ gradle ] ++ nativeBuildInputs;

    gradleFlags = [
      "-Dfile.encoding=utf-8"
      "-Dorg.gradle.java.home=${javaPackage}"
      "-Pix.mavenRepository=file://${mavenRepo}"
    ]
    ++ gradleFlags;

    gradleInitScript = localMavenInitScript;

    preConfigure = ''
      # shell
      ${preConfigure}
      rm -rf .gradle build
    '';

    installPhase =
      if installPhase == null then
        ''
          runHook preInstall

          install -Dm444 ${lib.escapeShellArg jarPath} "$out"

          runHook postInstall
        ''
      else
        installPhase;

    meta = meta // {
      sourceProvenance = (meta.sourceProvenance or [ ]) ++ [
        lib.sourceTypes.fromSource
        lib.sourceTypes.binaryBytecode
      ];
    };
  }
)
