{
  id = "claude-code-debug";
  packageSet = true;
  flake = true;
  # Debug helper, not something to shadow anything in nixpkgs.
  overlay = false;
}
