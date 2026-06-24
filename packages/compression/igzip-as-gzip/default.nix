{
  ix,
  lib,
  symlinkJoin,
  isa-l,
}:
let
  writeBashApplication = ix.writeBashApplication ix.pkgs;

  clampLevels = ''
    args=()
    for arg in "$@"; do
      case "$arg" in
        -[4-9]) args+=("-3") ;;
        *) args+=("$arg") ;;
      esac
    done
  '';

  mkWrapper =
    {
      name,
      flags ? [ ],
      description,
    }:
    writeBashApplication {
      inherit name;
      runtimeInputs = [ isa-l ];
      text = ''
        ${clampLevels}
        exec igzip ${lib.escapeShellArgs flags} "''${args[@]}"
      '';
      meta.description = description;
    };

  gzip = mkWrapper {
    name = "gzip";
    description = "gzip-compatible wrapper backed by ISA-L igzip";
  };

  gunzip = mkWrapper {
    name = "gunzip";
    flags = [ "-d" ];
    description = "gunzip-compatible wrapper backed by ISA-L igzip";
  };

  zcat = mkWrapper {
    name = "zcat";
    flags = [
      "-d"
      "-c"
    ];
    description = "zcat-compatible wrapper backed by ISA-L igzip";
  };
in
symlinkJoin {
  name = "igzip-as-gzip";
  paths = [
    gzip
    gunzip
    zcat
  ];
  meta = {
    description = "Drop-in gzip/gunzip/zcat replacement backed by ISA-L igzip";
    mainProgram = "gzip";
  };
}
