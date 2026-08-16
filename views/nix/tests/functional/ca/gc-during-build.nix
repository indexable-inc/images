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
rec {
  # A fast CA dependency. The seed makes both the derivation and its
  # output content unique per test run (CA outputs dedup on content, so
  # a stale output from a previous run must not be mistaken for ours).
  dep = mkCADerivation {
    name = "gc-during-build-dep";
    inherit seed;
    buildCommand = ''
      mkdir $out
      echo "dep-$seed" > $out/content
    '';
  };

  # An independent derivation that blocks on a fifo, keeping the
  # top-level build in flight (and its resolution unperformed) while the
  # test runs the garbage collector.
  blocker = mkCADerivation {
    name = "gc-during-build-blocker";
    inherit seed fifo;
    buildCommand = ''
      # Wait for the test driver to unblock us.
      cat $fifo
      mkdir $out
      echo "blocker-$seed" > $out/content
    '';
  };

  # Consumes both. Its resolved derivation (which would reference the
  # dep's output path) cannot be computed until the blocker finishes, so
  # nothing in the store references the dep's freshly built output when
  # the GC runs.
  top = mkCADerivation {
    name = "gc-during-build-top";
    inherit seed dep blocker;
    buildCommand = ''
      mkdir $out
      cat $dep/content $blocker/content > $out/content
    '';
  };
}
