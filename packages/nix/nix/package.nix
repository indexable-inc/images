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
  # `autoUpdate = false` in lib/fork-packages.nix keeps the daemon source out of
  # scheduled fork-sync. The updater only resolves the bootstrap action's
  # explicitly requested source ref into its generated lock; it does not move
  # the daemon version.
  id = "nix-ix";
  packageSet = true;
  flake = true;
  overlay = false;
  passthruTests = true;
  updateScript = true;
  # RFC 0009 cross lane (#3585): on a Linux build host, also expose nix-ix
  # cross-compiled to Darwin (default target aarch64-apple-darwin) so the
  # darwin cache lane substitutes the fork daemon instead of cold-building
  # the modular C++ closure on a Mac. default.nix reads the `ix.cross`
  # signal and swaps the component scope to the nixpkgs Linux -> Darwin
  # cross scope (lib/darwin/nixpkgs-cross.nix).
  cross = true;
}
