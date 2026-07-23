{
  id = "claude-code-rainbow";
  packageSet = true;
  flake = true;
  # Flake-output + index package set only, deliberately NOT a nixpkgs overlay:
  # this is a POC wrapper of the same unfree binary, not something that should
  # shadow anything in `pkgs`.
  overlay = false;
}
