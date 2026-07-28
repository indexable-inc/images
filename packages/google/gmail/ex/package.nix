{
  id = "google-gmail-ex";
  inRustWorkspace = true;
  # The NIF library comes from the shared cargo-unit workspace graph;
  # default.nix assembles the mix package (generated lib/ + priv/ + the
  # ExUnit suite) and compiles it into the loadable `:gmail_ex` OTP app
  # that ix-mcp-ex points `IX_MCP_GMAIL_EX` at.
  packageSet = true;
  # Gate the suite as `checks.<system>.google-gmail-ex-run`.
  passthruTests = {
    prefix = "google-gmail-ex";
  };
}
