/**
Composable `ix.dev.profiles.*` language/tool stacks.

Keep these as ordinary NixOS module fragments so profiles compose through the
normal `environment.systemPackages` merge path instead of making templates
hand-maintain long package lists.
*/
{
  config,
  ix,
  lib,
  pkgs,
  ...
}: let
  cfg = config.ix.dev.profiles;

  inherit (cfg) rust;
  rustToolchain = ix.rustToolchainFor pkgs {
    inherit
      (rust)
      channel
      version
      components
      targets
      profile
      ;
  };
in {
  imports = [./options.nix];

  config = lib.mkIf rust.enable {
    environment.systemPackages = [rustToolchain] ++ rust.packages;

    environment.variables = lib.mkIf rust.setEnvironment {
      RUST_BACKTRACE = lib.mkDefault "1";
      RUST_SRC_PATH = lib.mkDefault "${rustToolchain}/lib/rustlib/src/rust/library";
    };
  };
}
