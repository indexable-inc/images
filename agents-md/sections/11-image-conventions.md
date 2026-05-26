## Image conventions

An image is an independent NixOS system closure packaged as an OCI archive:
systemd as PID 1, no kernel, no bootloader. Images are not stacked at runtime;
layering is a build and registry storage concern.

Design for ix VM assumptions. Disks can grow very large, snapshots are normal,
and nodes can have substantial CPU and memory. Use limits to contain runaway
services, preserve rollback, and keep operations legible. Avoid shrinking useful
operator tooling merely to save a small closure delta.

Do not add at-rest encryption inside images as a default. ix storage deduplicates
guest blocks, and guest-side encryption turns identical data into random bytes.
If a workload has a real compliance requirement against the host, name that
requirement and design a separate channel for it.

Treat a root process inside the VM as fully capable inside the guest. Anything
that must hold against that process belongs outside the VM: host credentials,
registry-write tokens, snapshot authority, source-switch authority, and hard
network containment.

Use image networking for cooperative guest intent. Per-port firewall rules,
service frontends, and local mTLS belong in the image or a gateway VM. Policy
that must resist a compromised guest belongs in a router, gateway, group
boundary, or host-side primitive the guest cannot edit.

All images target `x86_64-linux`. Host-visible flake package namespaces may
exist for developer systems, but image derivations still build Linux systems.
Use generic nixpkgs packages when possible so upstream caches substitute.
Service-specific hardware tuning belongs in the module where the operator can
see the tradeoff.

Use topology for same-protocol public port conflicts. Put services that need the
same natural port in separate nodes, use an explicit alternate port, put a real
frontend in front of them, or create a true namespace boundary. Runtime "pick any
free port" behavior makes docs, firewalls, health checks, and fleet plans lie.

Do not assume registry images are public. System namespaces may publish public
bootstrap images; user namespaces default to private and should behave like
not-found for other users. Debug access before treating a pull failure as an
outage.

Platform-wide defaults have two homes. System posture lives in
[`lib/ix-platform.nix`](lib/ix-platform.nix). Operator ergonomics and shared CLI
tools live in [`modules/profiles/base/`](modules/profiles/base/). Use
`lib.mkDefault` when an unusual image might need a one-line override.

Add a new image by adding a NixOS module at
`images/<category>/<name>/default.nix`. Discovery exposes the package on the
next eval. A versioned image keeps variants in a sibling `versions.nix`, with one
default variant and one small data record per version.

Images and presets should use one coherent `services.<name>` block per service.
Nest sub-options under that block so the configuration reads like the service
shape rather than a scatter of dotted assignments.

Presets should own intent. Artifact URLs, hashes, generated metadata, and broad
catalog data belong to the nearest update mechanism that can refresh them
mechanically. A preset may show a local or private artifact when the example is
about that override.

