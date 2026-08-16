with import ./config.nix;

let
  mkBlake3CADerivation =
    args:
    mkDerivation (
      {
        __contentAddressed = true;
        outputHashMode = "recursive";
        outputHashAlgo = "blake3";
      }
      // args
    );
in

rec {
  dep = mkDerivation {
    name = "blake3-dep";
    buildCommand = ''
      mkdir -p $out
      echo "dependency contents" > $out/hello
    '';
  };

  # A BLAKE3 content-addressed output with no references at all, so that its
  # content address is directly comparable to a BLAKE3 hash of its NAR.
  plain = mkBlake3CADerivation {
    name = "blake3-plain";
    buildCommand = ''
      echo "plain blake3 payload" > $out
    '';
  };

  # A BLAKE3 content-addressed output that refers both to another store path
  # and to itself. Only the reference-bearing store path scheme can name such
  # an output, and that scheme used to be reachable by SHA-256 alone.
  selfRef = mkBlake3CADerivation {
    name = "blake3-selfref";
    buildCommand = ''
      mkdir -p $out
      echo ${dep}/hello > $out/dep
      echo $out > $out/self
    '';
  };

  dependent = mkBlake3CADerivation {
    name = "blake3-dependent";
    buildCommand = ''
      mkdir -p $out
      cat ${selfRef}/self
      echo ${selfRef} > $out/dep
    '';
  };
}
