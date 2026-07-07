{
  id = "unibind-conformance-jvm";
  inRustWorkspace = true;
  # Nothing to ship: the crate exists to prove the generated JVM surface
  # behaves (envelope decoding, mirror layouts, exception mapping, panic
  # containment) from both Java and Kotlin, and default.nix wraps that proof
  # in a runnable check. The packageSet entry only exists so the registry
  # resolves its `passthru.tests` for the CI gates below.
  packageSet = true;
  # Gate the runners as `checks.<system>.unibind-conformance-jvm-run`, next
  # to the crate's own unit gates (clippy over the expanded glue included).
  passthruTests = {
    prefix = "unibind-conformance-jvm";
  };
}
