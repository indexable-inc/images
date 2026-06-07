{ pkgs, ... }:
{
  # This is the line to edit. Add or remove packages, then run `ix up` again
  # and ix rebuilds the closure and activates it on the running
  # VM in place, the same contract as `nixos-rebuild switch`.
  #
  # This is an ordinary NixOS module: `services.*`, `users.*`, `systemd.*`, and
  # any module shipped by the ix base image all work here too.
  environment.systemPackages = [
    pkgs.htop
  ];
}
