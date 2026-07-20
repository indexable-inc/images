{
  lib,
  writeNushellApplication,
}:
/**
Shim `xcrun`/`xcode-select` to a pinned nixpkgs `apple-sdk` for zig builds on
darwin.

zig 0.15's darwin SDK probe (`std.zig.system.darwin.getSdk` /
`isSdkInstalled`) shells out to `xcrun --show-sdk-path` and `xcode-select
--print-path`, neither of which exists in the Nix build sandbox. Every
zig-built ghostty artifact (`libghostty-vt`, the `ghostty` package) needs the
same two-command shim pointed at the same pinned SDK, so it lives here once
rather than copied per package.

Returns `{ appleSdk, appleSdkRoot, darwinSdkInputs }`: `darwinSdkInputs` is
the shim packages to add to `nativeBuildInputs` on darwin (empty list
elsewhere), `appleSdk`/`appleSdkRoot` are the pinned SDK derivation and its
`MacOSX.sdk` path for `SDKROOT`/`DEVELOPER_DIR` and `buildInputs`.
*/
pkgs: let
  isDarwin = pkgs.stdenv.hostPlatform.isDarwin;
  appleSdk = pkgs.apple-sdk_14;
  appleSdkRoot = "${appleSdk}/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk";

  # `--wrapped main [...args]` lets the shim swallow every flag zig passes
  # (`xcrun --sdk macosx --show-sdk-path`, `xcode-select --print-path`) and
  # just echo the pinned path. writeShellScriptBin is banned (no nu-check, no
  # declared deps), so these go through the checked Nushell writer.
  xcrunShim = writeNushellApplication pkgs {
    name = "xcrun";
    text = ''
      # nu
      def --wrapped main [...args] { print "${appleSdkRoot}" }
    '';
  };
  xcodeSelectShim = writeNushellApplication pkgs {
    name = "xcode-select";
    text = ''
      # nu
      def --wrapped main [...args] { print "${appleSdk}" }
    '';
  };
in {
  inherit appleSdk appleSdkRoot;
  darwinSdkInputs = lib.optionals isDarwin [xcrunShim xcodeSelectShim];
}
