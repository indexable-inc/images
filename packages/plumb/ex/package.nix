{
  id = "plumb-ex";
  inRustWorkspace = true;
  # The NIF library comes from the shared cargo-unit workspace graph;
  # default.nix assembles the mix package (generated lib/ + priv/ + the
  # ExUnit suite) and runs it offline. The packageSet entry exists so the
  # registry resolves passthru.tests for the CI gate below.
  packageSet = true;
  # Gate the suite as `checks.<system>.plumb-ex-run`.
  passthruTests = {
    prefix = "plumb-ex";
  };
}
