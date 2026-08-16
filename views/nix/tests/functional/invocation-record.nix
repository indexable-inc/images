with import ./config.nix;

# Two derivations with different builder durations, so the record has
# per-derivation timings that can be told apart.
mkDerivation {
  name = "invocation-record-root";
  dep = mkDerivation {
    name = "invocation-record-dep";
    buildCommand = ''
      sleep 2
      echo dep > $out
    '';
  };
  buildCommand = ''
    echo root > $out
  '';
}
