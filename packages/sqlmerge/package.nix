{
  id = "sqlmerge";
  packageSet = true;
  flake = true;
  # The base profile wires sqlmerge into git as the merge driver for SQLite
  # files, so thread it into the nixpkgs overlay for modules to take as
  # `pkgs.sqlmerge`.
  overlay = true;
  inRustWorkspace = true;
  mirror.repo = "indexable-inc/sqlmerge";
  passthruTests = true;
}
