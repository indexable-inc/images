/**
The demo pod image, built by dockerTools instead of pulled from a registry.

Every node preloads this image (node.nix) because the scheduler may place
pods on any of them; the manifest references it by `imageName:imageTag`.
*/
{
  ix,
  lib,
  pkgs,
}: let
  ports = import ./ports.nix;

  serve = ix.writePythonApplication pkgs {
    name = "whoami-serve";
    src = ./whoami_serve.py;
    args = [(toString ports.app)];
  };
in
  pkgs.dockerTools.buildLayeredImage {
    name = "whoami";
    tag = "nix";
    config.Entrypoint = [(lib.getExe serve)];
  }
