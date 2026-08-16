with import ./config.nix;

let
  mkCADerivation =
    args:
    mkDerivation (
      {
        __contentAddressed = true;
        outputHashMode = "recursive";
        outputHashAlgo = "sha256";
      }
      // args
    );

  failingLeaf =
    name: sentinel:
    mkCADerivation {
      inherit name;
      buildCommand = ''
        echo ${sentinel} >&2
        exit 42
      '';
    };

  firstLeaf = failingLeaf "ca-failing-first" "CA_FAILURE_FIRST";
  secondLeaf = failingLeaf "ca-failing-second" "CA_FAILURE_SECOND";
in
{
  resolvedFailure = firstLeaf;

  failFast = mkCADerivation {
    name = "ca-failure-root";
    buildCommand = ''
      cat ${firstLeaf}
      touch $out
    '';
  };

  keepGoing = mkCADerivation {
    name = "ca-failure-root-keep-going";
    buildCommand = ''
      cat ${firstLeaf} ${secondLeaf}
      touch $out
    '';
  };
}
