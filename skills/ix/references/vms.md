# Virtual Machines

VMs boot in ~1 second from any OCI image. Full VMs with their own kernel; PID 1 is your entrypoint; systemd works.

> [!NOTE]
> ~1s is median for cached images in a warm region. First pull of a large image is slower.

## Create

Run `ix --help` for create syntax. Images must be fully qualified OCI references (include the registry).

## Snapshots

Captures memory and disk. Snapshots are immutable.

## Groups

East-west networking groups for VM-to-VM connectivity.

## NixOS

`ix switch` applies a new NixOS system configuration to a running VM, the same contract as `nixos-rebuild switch`: the VM keeps running and only its system generation changes. The target is a Nix installable, a `.drv` path, or a `/nix/store` system path.

This is native, not remote-shell puppeteering. For the default same-VM path, your source tree is uploaded to ix and the `nix build` plus `switch-to-configuration switch` run server-side on ix infrastructure; only the build/activate output streams back.

`ix up` is the declarative front-end over create + switch: it creates the VM from a NixOS base image (`ix/base:latest`) if it does not exist yet, then switches it to the freshly built system. Re-running `ix up` converges the VM to the current configuration.

The [`nixos-switch`](../../../examples/nixos-switch/) example is the smallest fork-and-go version of this loop: one NixOS node, one package list to edit, `ix up` to converge.
