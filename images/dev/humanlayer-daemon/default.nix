# A bootable ix VM that runs the HumanLayer (riptide) remote daemon.
#
# Builds on the dev base image so the daemon has the agent CLIs and build
# toolchain a remote HumanLayer host needs to drive coding sessions. The daemon
# itself is the `services.humanlayer` module.
#
# Auth: drop a launch token at `/run/secrets/humanlayer/launch-token` before
# boot (mint with `humanlayer api auth daemon launch-token create` on an
# authenticated host, or copy the launch command from app.humanlayer.com). The
# token is read at service start and never baked into the image.
{
  ix,
  pkgs,
  ...
}:
{
  imports = [ (ix.paths.root + "/images/dev/development-base") ];

  # development-base sets the name with mkOptionDefault, so this plain
  # assignment overrides it. Named distinctly from the `humanlayer` CLI package
  # so the two do not collide in the flake package set.
  ix.image.name = "humanlayer-daemon";

  environment.systemPackages = [ pkgs.humanlayer ];

  services.humanlayer = {
    enable = true;
    launchTokenFile = "/run/secrets/humanlayer/launch-token";
  };
}
