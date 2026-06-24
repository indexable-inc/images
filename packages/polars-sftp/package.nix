{
  id = "polars-sftp";
  flake = true;
  overlay = false;
  packageSet = true;
  # Gate the strict Python type/annotation check (default.nix passthru.tests.pyStrict)
  # in CI as `checks.<system>.polars-sftp-pyStrict`.
  passthruTests = {
    prefix = "polars-sftp";
  };
}
