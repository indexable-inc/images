{
  id = "jj";
  packageSet = true;
  flake = true;
  overlay = false;
  # Puts jj's `passthru.tests` in `checks`/`ciChecks`. That set is exactly the
  # two clippy gates for the crates we own in this vendored workspace (see
  # `policy.clippy.packages` in default.nix); without the flag they are
  # derivations nothing builds, which is a lint gate that passes by never
  # running.
  passthruTests = true;
}
