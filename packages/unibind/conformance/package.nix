{
  id = "unibind-conformance";
  inRustWorkspace = true;
  # Nothing to ship: the crate exists to prove the generated Python surface
  # behaves (cancellation drops the future, streams pull, resources close,
  # buffers cross zero-copy), and default.nix wraps that proof in a runnable
  # check. The packageSet entry only exists so the registry resolves its
  # `passthru.tests` for the CI gate below.
  packageSet = true;
  # Gate the runner as `checks.<system>.unibind-conformance-run`.
  passthruTests = {
    prefix = "unibind-conformance";
  };
}
