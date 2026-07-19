{
  id = "btop";
  packageSet = true;
  flake = true;
  overlay = false;
  # Linux->Darwin cross lane (RFC 0009, #3584): CI cross-compiles the Mach-O
  # arm64 binary with the apple-sdk cross toolchain (clang + macOS SDK) and aliases it into
  # `packages.aarch64-darwin.btop`, so Macs substitute it from the cache
  # instead of the darwin cache-push lane building it natively.
  cross = true;
  # Joins `nix run .#update`: bump btop-src and regenerate the patch series via
  # passthru.updateScript (see default.nix / lib/fork-updater.nix).
  updateScript = true;
}
