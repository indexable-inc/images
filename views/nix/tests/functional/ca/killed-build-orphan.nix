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
in

{
  seed ? 0,
  outRecord,
  goFlag,
}:
{
  # A floating-CA derivation whose builder records its (non-chroot)
  # scratch output path, then fails until the test driver creates the
  # go-flag file. The failing first run tells the test driver the
  # deterministic scratch path at which to plant a killed-build orphan;
  # the second run must clear that orphan and succeed. The flag file is
  # runtime state, not part of the derivation, so both runs build the
  # same derivation and get the same scratch path.
  top = mkCADerivation {
    name = "killed-build-orphan";
    inherit seed outRecord goFlag;
    buildCommand = ''
      echo "$out" >> "$outRecord"
      if [ ! -e "$goFlag" ]; then
        exit 1
      fi
      mkdir -p $out
      echo "payload-$seed" > $out/c
    '';
  };
}
