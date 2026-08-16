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
  fifo,
}:
{
  # A floating-CA derivation whose builder writes its output and then
  # blocks, keeping the (non-chroot) scratch output path on disk while
  # the test driver runs the garbage collector.
  top = mkCADerivation {
    name = "gc-scratch-output";
    inherit seed fifo;
    buildCommand = ''
      mkdir $out
      echo "scratch-$seed" > $out/c
      # Block until the test driver has run the GC.
      cat $fifo
      echo done >> $out/c
    '';
  };
}
