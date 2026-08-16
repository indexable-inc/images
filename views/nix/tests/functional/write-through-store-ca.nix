with import ./config.nix;

# Floating content-addressed, which is the case that makes the realisation
# load-bearing rather than a nicety: the output path is a function of the bytes
# the build produced, so it cannot be computed from the derivation. A host
# holding this derivation and no realisation has no name to ask a cache for.
mkDerivation {
  name = "write-through-store-ca";
  builder = ./write-through-store-ca.builder.sh;
  __contentAddressed = true;
  outputHashMode = "recursive";
  outputHashAlgo = "sha256";
}
