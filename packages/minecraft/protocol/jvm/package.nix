{
  id = "mc-protocol-jvm";
  inRustWorkspace = true;
  # The cdylib must not inherit workspace-unified backend features from the
  # sibling Python bindings crate (pyo3's #[used] constructors would leave
  # it with undefined Python symbols at dlopen); resolve this crate's
  # dependency features from its own manifest via a dedicated `-p` cargo
  # invocation in the shared unit graph.
  isolatedFeatures = true;
  # Nothing to ship from here directly: the built cdylib is consumed by
  # `ix.unibind.build { crate = "mc-protocol-jvm"; ... }` in
  # ../../probe-kt/default.nix, which packages the generated Java class and
  # the native library into the Kotlin probe.
}
