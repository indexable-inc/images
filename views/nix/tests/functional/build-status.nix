with import ./config.nix;

# A derivation whose builder sleeps, so the build stays "running" long enough
# for `nix store builds` to observe its status file.
mkDerivation {
  name = "build-status-sleeper";
  buildCommand = ''
    sleep 30
    echo hi > $out
  '';
}
