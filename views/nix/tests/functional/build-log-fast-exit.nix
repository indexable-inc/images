with import ./config.nix;

# A builder that writes its diagnostic and exits immediately, plus enough
# siblings to keep the scheduler busy starting children instead of reading
# their output. See build-log-fast-exit.sh.
rec {
  failer = mkDerivation {
    name = "fast-exit-failer";
    buildCommand = ''
      echo "DIAGNOSTIC-LINE-1" >&2
      echo "DIAGNOSTIC-LINE-2" >&2
      echo "DIAGNOSTIC-LINE-3" >&2
      exit 1
    '';
  };

  all = mkDerivation {
    name = "fast-exit-all";
    inputs = [
      failer
    ]
    ++ builtins.genList (
      i:
      mkDerivation {
        name = "fast-exit-sibling-${toString i}";
        buildCommand = ''
          echo "sibling ${toString i} ran" >&2
          touch $out
        '';
      }
    ) 8;
    buildCommand = ''
      touch $out
    '';
  };
}
