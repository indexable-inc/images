# Registry metadata. The harness + ctl pair is a flake package (`nix build
# .#beamvm`, `index.packages.<sys>.beamvm`); the home-module that composes it
# into a portable service is exposed as `homeModules.beamvm` from flake.nix.
{
  id = "beamvm";
  packageSet = true;
  flake = true;
  overlay = false;
  passthruTests = true;
}
