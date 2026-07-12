{
  # Upstream NixOS/nix (nix-src input) with the in-repo patch series
  # (./patches) applied, built through nixpkgs' modular nix packaging so it is
  # a drop-in for the daemon version the fleet runs (2.34.7). Surfaced as
  # `pkgs.nix-ix` in the repo package set and as the `nix-ix` flake output.
  #
  # Deliberately NOT in the nixpkgs overlay: the derivation reads
  # `pkgs.nixVersions.nixComponents_2_34` as its base, so injecting this package
  # under the bare `nix` name would make it its own base (infinite recursion),
  # exactly as nix-eval-jobs / nix-output-monitor document for their overrides.
  #
  # `autoUpdate = false` in lib/fork-packages.nix keeps the base pinned by rev
  # and out of the scheduled fork-sync, so it is not wired to the routine
  # `nix run .#update` DAG (like clippy, and unlike codex/btop): the daemon
  # version moves only under a deliberate change. Hence no `updateScript` flag.
  id = "nix-ix";
  packageSet = true;
  flake = true;
  overlay = false;
  passthruTests = true;
}
