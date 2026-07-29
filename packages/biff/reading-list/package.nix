{
  id = "biff-reading-list";
  packageSet = true;
  flake = true;
  # The NixOS service module and the `ix apply` example both need the
  # application by name rather than by argument-threading, so it lands in
  # `pkgs`. Without this the example under examples/biff/reading-list would
  # have to take the package as a flake input, which is what made
  # lib/discovery.nix skip it.
  overlay = true;
  # Fans `passthru.tests.clojure` into a flake check, so the project's own
  # suite runs in CI instead of only on whoever last typed `clojure -M:test`.
  passthruTests.prefix = "clj-biff-reading-list";
}
