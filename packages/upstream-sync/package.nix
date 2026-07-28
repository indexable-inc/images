{
  id = "upstream-sync";
  packageSet = true;
  flake = true;
  overlay = false;
  inRustWorkspace = true;
  # Surfaces the crate's cargo-unit test cases as `upstream-sync-*` checks
  # (explicit prefix: the default would be the misleading `rust-upstream-sync`).
  passthruTests = {prefix = "upstream-sync";};
}
