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

  parseArtifacts =
    component: remainingLines:
    let
      go =
        activeArtifact: todo:
        if todo == [ ] then
          [ ]
        else
          let
            line = builtins.head todo;
            rest = builtins.tail todo;
            artifactName = attrFromLine "name" line;
            sha256 = attrFromLine "value" line;
          in
          if lib.hasInfix "</component>" line then
            [ ]
          else if artifactName != null then
            go artifactName rest
          else if activeArtifact != null && sha256 != null then
            [
              (
                component
                // {
                  inherit sha256;
                  file = activeArtifact;
                }
              )
            ]
            ++ go null rest
          else
            go activeArtifact rest;
    in
    go null remainingLines;

  parseComponents =
    current: remainingLines:
    if remainingLines == [ ] then
      [ ]
    else
      let
        line = builtins.head remainingLines;
        rest = builtins.tail remainingLines;
        group = attrFromLine "group" line;
        name = attrFromLine "name" line;
        artifactVersion = attrFromLine "version" line;
      in
      if lib.hasInfix "<component " line then
        parseComponents {
          inherit group name;
          version = artifactVersion;
        } rest
      else if current != null && lib.hasInfix ''<artifact name="'' line then
        parseArtifacts current remainingLines ++ parseComponents null remainingLines
      else
        parseComponents current rest;

  artifacts = parseComponents null lines;

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
        hash = "sha256:${artifact.sha256}";
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
