{
  id = "unibind-conformance-jvm";
  inRustWorkspace = true;
  # The cdylib must not inherit workspace-unified backend features of the
  # sibling conformance crates (pyo3's #[used] constructors would leave it
  # with undefined Python symbols at dlopen); resolve this crate's
  # dependency features from its own manifest via a dedicated `-p` cargo
  # invocation in the shared unit graph.
  isolatedFeatures = true;
  # Nothing to ship: the crate exists to prove the generated Java class
  # behaves against the real cdylib, and default.nix wraps that proof in a
  # runnable javac/java check. The packageSet entry only exists so the
  # registry resolves its `passthru.tests` for the CI gate below.
  packageSet = true;
  # Gate the suite as `checks.<system>.unibind-conformance-jvm-run`.
  passthruTests = {
    prefix = "unibind-conformance-jvm";
  };
}
