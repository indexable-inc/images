{ pkgs, ... }:
{
  ix.image = {
    name = "ix/test-cluster-bootstrap";
    tag = "zstd-tools-2026-05-12";
  };

  networking.hostName = "test-cluster-bootstrap";

  nix.settings = {
    experimental-features = [
      "nix-command"
      "flakes"
    ];
    substituters = [
      "http://cache.ix.dev:8501"
      "https://cache.nixos.org/"
    ];
    trusted-public-keys = [
      "hil-compute-1:eu2JX3qkNaxdO0/ane+bTmOIKOtR5P/quLlTHBqqIpM="
      "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY="
    ];
  };
}
