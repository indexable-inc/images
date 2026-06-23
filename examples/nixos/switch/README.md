# NixOS switch

The smallest fork-and-go example of the native `ix up` loop: boot a NixOS VM
from the `ix/base` image, add a package, and watch ix rebuild and activate the
new system on the running VM in place.

This is the native path to switch a NixOS VM via ix infrastructure. Your source
tree is uploaded to ix, the `nix build` and `switch-to-configuration switch` run
server-side on ix (not as remote-shell commands driven from your laptop), and
only the result streams back. Re-running converges the VM to the current
configuration, the same contract as `nixos-rebuild switch`.

## Run

```sh
ix up .#devbox
```

The first run creates `devbox` from `ix/base:latest` and activates this
configuration on it.

## The loop

1. Edit [`configuration.nix`](configuration.nix): add a package to
   `environment.systemPackages` (try `pkgs.ripgrep`).
2. Run `ix up .#devbox` again. ix uploads the change, builds the new closure on ix, and
   switches the running VM to it.
3. `ix shell devbox` and confirm the new package is on `PATH`.

The VM keeps running across switches: only its system generation changes,
nothing is recreated.

## Shape

- [`flake.nix`](flake.nix) is the native `ix up` entrypoint. It exposes
  `nixosConfigurations.devbox`, which `ix up .#devbox` resolves to the NixOS
  system closure.
- [`ix.nix`](ix.nix) keeps the one-node fleet definition reused by the repo
  example wrappers.
- [`configuration.nix`](configuration.nix) is the NixOS module you edit.

## Fork it

Copy this directory into your own repo and keep `flake.nix` as the entrypoint. The switch path needs no admin rights: it builds and activates your own system onto your own VM.

## Scope

This builds on the target VM itself, the `ix up` default. Building
on a separate per-user builder VM (`--build-vm`) is a follow-up; the same-VM
path shown here is the native switch primitive.
