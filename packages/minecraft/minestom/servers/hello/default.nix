{
  ix,
  lib,
  pkgs ? ix.pkgs,
}: let
  fs = lib.fileset;
  src = fs.toSource {
    root = ./.;
    fileset = fs.intersection (fs.gitTracked ./.) ./.;
  };
in
  ix.buildGradleFatJar pkgs {
    pname = "minestom-hello";
    version = "0.1.0";
    inherit src;
    verificationMetadata = ./gradle/verification-metadata.xml;
  }
