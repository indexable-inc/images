{
  id = "unibind-conformance-ex";
  inRustWorkspace = true;
  # Nothing to ship: the crate exists to prove the generated Elixir surface
  # behaves (async replies arrive, a caller exit cancels the in-flight
  # future, GC runs resource destructors, streams respect demand), and
  # default.nix wraps that proof in a runnable ExUnit check. The packageSet
  # entry only exists so the registry resolves its `passthru.tests` for the
  # CI gate below.
  packageSet = true;
  # Gate the suite as `checks.<system>.unibind-conformance-ex-run`.
  passthruTests = {
    prefix = "unibind-conformance-ex";
  };
}
