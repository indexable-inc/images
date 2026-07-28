{
  # snix's `default` CLI built through cargo-unit. Surface it as `pkgs.snix` in
  # the repo package set and as the `snix` flake output. GPL-3.0; builds on
  # Linux and macOS (meta.platforms = unix). NOT inRustWorkspace: snix is an
  # external fetched Cargo workspace, not a member of this repo's workspace.
  id = "snix";
  flake = true;
  overlay = false;
  packageSet = true;
}
