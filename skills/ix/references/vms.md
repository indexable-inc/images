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

`ix switch` applies a new NixOS system configuration to a running VM. Target can be a Nix installable, a `.drv` path, or a `/nix/store` system path.
