{
  id = "astlog-py";
  inRustWorkspace = true;
  # No wheel to redistribute (the cdylib is bundled into ix-mcp), but default.nix
  # exposes the strict Python type/annotation check; a packageSet entry lets the
  # registry resolve its `passthru.tests` for the CI gate below.
  packageSet = true;
  # Gate the strict Python check as `checks.<system>.astlog-py-pyStrict`.
  passthruTests = {
    prefix = "astlog-py";
  };
}
