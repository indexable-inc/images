{
  ix,
  lib,
  pkgs ? ix.pkgs,
}: let
  fs = lib.fileset;
  minestomRoot = ix.paths.packagesRoot + "/minecraft/minestom";
  src = fs.toSource {
    root = minestomRoot;
    fileset = fs.intersection (fs.gitTracked minestomRoot) minestomRoot;
  };
in
  ix.buildGradleFatJar pkgs {
    pname = "minestom-hello";
    version = "0.1.0";
    inherit src;
    gradleBuildTask = ":servers:hello:jar";
    jarPath = "servers/hello/build/libs/minestom-hello-0.1.0.jar";
    verificationMetadata = minestomRoot + "/gradle/verification-metadata.xml";
  }
