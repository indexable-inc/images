{
  id = "upstream-sync";
  packageSet = true;
  flake = true;
  overlay = false;
  # Surfaces passthru.tests.lifecycle as the `upstream-sync-lifecycle` check
  # (explicit prefix: the default would be the misleading `rust-upstream-sync`).
  passthruTests = {prefix = "upstream-sync";};
}
