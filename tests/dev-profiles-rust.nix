# Guard for `ix.dev.profiles.rust`: the option must be reachable from a
# consumer's module, not merely declared.
#
# It was not. `lib/dev/profiles.nix` reads `ix.rustToolchainFor` off the module
# argument, and that argument is `sharedHelpers` in lib/default.nix, which
# carried `rustWorkspace` and `rustWorkspaceFor` and not the toolchain builder.
# So every `ix.dev.profiles.rust.enable = true` died with `attribute
# 'rustToolchainFor' missing`, and nothing in the tree noticed because nothing
# in the tree enabled it: the option shipped documented and unusable, with its
# own description naming the function that was absent.
#
# Eval-only, so it costs nothing, and it exercises the real path: an image
# config built the way `mkDev` builds one, taking the same `ix` handle a
# consumer's module receives. Asserting against `ix.rustToolchainFor` directly
# would pass with the bug in place, because the tests get the full lib rather
# than the curated module surface.
{
  lib,
  pkgs,
  ix,
  paths,
}: let
  configWithRust = ix.evalImageConfig {
    modules = [
      (paths.root + "/lib/dev/options.nix")
      (paths.root + "/lib/dev/profiles.nix")
      {
        ix.dev.profiles.rust.enable = true;
        ix.image.name = "dev-profiles-rust-guard";
      }
    ];
  };

  packageNames = map (package: package.pname or package.name or "") configWithRust.environment.systemPackages;

  # The toolchain arrives as one wrapped derivation, so this asserts the
  # profile reached `environment.systemPackages` at all. The failure being
  # guarded against is an eval error one line earlier, not a wrong package.
  carriesAToolchain = lib.any (name: lib.hasInfix "rust" (lib.toLower name)) packageNames;
in
  assert lib.assertMsg carriesAToolchain ''
    ix.dev.profiles.rust.enable = true produced no rust package in
    environment.systemPackages. Rendered: ${lib.generators.toPretty {} packageNames}

    lib/dev/profiles.nix builds the toolchain through the `ix` module
    argument, which is `sharedHelpers` in lib/default.nix. If this failed with
    a missing attribute rather than this message, a name that file reads was
    dropped from that surface.
  '';
    pkgs.runCommand "dev-profiles-rust-guard" {__structuredAttrs = true;} "touch $out"
