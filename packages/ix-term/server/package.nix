{
  id = "ix-term-server";
  # The server spawns PTY login shells and publishes pts paths under
  # /run/ix-term for the `ixterm` CLI, so the packaged artifact is linux-only
  # (the crate itself stays in the cargo workspace on darwin for local dev).
  # `overlay` is on so the NixOS module can reference `pkgs.ix-term-server`
  # via `mkPackageOption`.
  packageSet.systems = ["x86_64-linux" "aarch64-linux"];
  flake.systems = ["x86_64-linux" "aarch64-linux"];
  overlay.systems = ["x86_64-linux" "aarch64-linux"];
  inRustWorkspace = true;
  passthruTests = true;
}
