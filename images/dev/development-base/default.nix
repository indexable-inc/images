# NixOS development base image: agent CLIs plus build tools. The auto-enabled
# base profile supplies shells, editors, and debugging utilities.
{ pkgs, ... }:
{
  ix.image.name = "development-base";

  environment.systemPackages = [
    pkgs.llm-agents.claude-code
    pkgs.llm-agents.codex

    pkgs.cmake
    pkgs.gcc
    pkgs.gnumake
    pkgs.ninja
    pkgs.nodejs
    pkgs.pkg-config
    pkgs.python3
    pkgs.rustup
  ];
}
