# Build a `passthru.updateScript` for a de-forked package: bump its upstream
# base (`nix flake update <input>`) then regenerate its patch series
# (`rebase-patches <name>`), so the fork joins the registry-discovered
# `nix run .#update` DAG like every other updater (one node per fork, no
# hardcoded fork list in lib/per-system.nix). Same "human reviews the bump, the
# tool regenerates bytes" posture as lib/util/pins.nix's mkUpdater. The
# behavior lives in the pin-update engine's `fork` mode
# (packages/nix/pin-update); this file only renders the spec.
#
# clippy is deliberately NOT wired here: it is nightly-toolchain-coupled and
# rev-pinned, so it must not free-float under the routine updater (see
# flake.nix); its patches are rebased by hand alongside a toolchain bump via
# `nix run .#rebase-patches -- clippy`.
{
  pinUpdate,
  nix,
  rebasePatches,
}:
# `name` selects the fork (matches lib/fork-packages.nix and the rebase arg);
# `input` is the flake.lock input the bump advances.
{
  name,
  input,
}:
pinUpdate.mkUpdateScript {
  name = "${name}-fork-update";
  description = "Bump the ${input} base and regenerate ${name}'s patch series";
  runtimeInputs = [
    nix
    rebasePatches
  ];
  spec = {
    mode = "fork";
    fork = name;
    inherit input;
  };
}
