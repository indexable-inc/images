# A derivation whose output hard-references a store object the EVALUATOR added
# rather than a build produced. `builtins.path` copies the source into the store
# at evaluation time, so it has no deriver: no build of it ever runs, and nothing
# publishes it except as part of some output's closure. nixpkgs'
# `separateDebugInfo` produces exactly this shape, a symlink from the `-debug`
# output into the source tree, which is how an unpublished reference stopped
# every CI job on the fleet (ENG-12418).
with import ./config.nix;

mkDerivation {
  name = "write-through-store-reference";
  builder = ./write-through-store-reference.builder.sh;
  evaluatorAddedSource = builtins.path {
    path = ./write-through-store.builder.sh;
    name = "write-through-store-evaluator-added-source";
  };
}
