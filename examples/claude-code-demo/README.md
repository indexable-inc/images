# Claude Code Demo

Two VMs:

- `demo`: a shellable ix VM with `btop`, Linux source in `/src/linux`, and a tiny Svelte status page on port 80.
- `minecraft`: a Fabric server pinned to `26.2-snapshot-6`, creative mode, flat world, public Java port 25565.

## Run

```bash
nix run .#claude-code-demo-plan
nix run .#claude-code-demo-up
```

`up` builds the fleet's OCI image derivations, uploads the archives with the ix
CLI, and creates or starts the VMs from those uploaded images. Use this while
`ix switch` source switching is still unstable.

## Demo Flow

Open the first VM shell and show the machine:

```bash
ix shell demo
btop
cd /src/linux
```

Open the `demo` web URL before starting the build. The page is hosted inside the VM and shows live CPU usage out of 64 cores, memory usage out of 256 GiB, disk usage out of 1 PiB, and current cost per second. Start with the VM doing almost nothing to show the very low cost per second, then build the Linux kernel and watch the web view update as CPU and memory usage increase. The cost is dynamic: the VM is charged for the resources it is actually using, not a fixed machine size.

```bash
make -j$(nproc) defconfig bzImage
```

Then use Minecraft:

```bash
ix shell minecraft
```

Join the server, take a snapshot, blow up the flat creative world with TNT, then switch or restore to show the stateful VM workflow. `switch` snapshots existing nodes before applying the new NixOS systems; `replace` is only for recreating VMs from OCI images.

## Behind The Hood Slides

1. Plan: `nix run .#claude-code-demo-plan` evaluates the fleet and shows the two target VMs, their systems, and their exposed ports.
2. Switch: `nix run .#claude-code-demo-switch` creates missing VMs, snapshots existing ones, then activates the NixOS systems in dependency order.
3. Demo VM: `ix shell demo` opens the build box with Linux source already cloned, live stats served by nginx, and enough CPU/memory/disk to make the machine feel real.
4. Web view: open the demo URL first and show the idle VM costing very little per second.
5. Dynamic pricing: start the kernel build, then return to the web view to show CPU and memory usage rising and cost per second increasing with actual resource usage.
6. Minecraft VM: `ix shell minecraft` shows the second VM is managed by the same fleet, but runs a different workload: a Fabric server with a pinned snapshot jar and declarative `server.properties`.
7. Stateful moment: snapshot, break the world with TNT, then switch or restore. The point is that normal updates preserve VM state; replacement images are only for explicit recreation.
8. Wrap: the same source tree defines both machines, their packages, service config, exposed ports, and rollout behavior.
