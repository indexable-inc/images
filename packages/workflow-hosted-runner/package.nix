{
  id = "workflow-hosted-runner";
  packageSet = true;
  # Reachable as `nix build .#workflow-hosted-runner`, so the refusal can be
  # reproduced locally without going through the whole gate.
  flake = true;
  # Nothing outside this repository consumes a lint over this repository's own
  # workflows.
  overlay = false;
}
