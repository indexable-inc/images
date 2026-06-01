# Philosophy

## Own the metal

No cloud provider underneath. Cloud margin is real money at agent scale; cutting it is what makes per-second billing and deduplicated forks cheap.

> [!NOTE]
> We lease hosts long-term from a bare-metal provider. No cloud VM layer under ours, no shared hypervisor, no networked block store, direct-attached NVMe, real public IPs. See [hardware](hardware.md) and [reliability](reliability.md#business-continuity).

Specs in [hardware](hardware.md). Rates in [pricing](pricing.md).

## Primitives over prebaked stacks

Small primitives, you compose. We avoid opinionated higher-level products even when they'd ship faster for the common case.

Example: desktop / GUI. Prebaked screenshot + click surfaces are useful on day one and a constraint on day thirty. When your use case drifts, you're blocked on someone else's roadmap. See [desktop-gui](desktop-gui.md).

Same thinking elsewhere:

- **Networking.** Raw TCP/UDP, direct public IP. No load balancer or port mapping. See [network](network.md).
- **Init.** PID 1 is your entrypoint. Systemd works. See [vms](vms.md).
- **Scheduling.** You decide how to fork, snapshot, and restore.

## LLM-first

Docs are short on purpose. The [TypeScript](sdk/typescript.md) and [Python](sdk/python.md) SDK sources are small and are the spec.

## Browser-native

The SDK runs in a browser and talks directly to a VM over WebTransport, skipping our API server. Same SDK works in Node, Bun, and Python. See [browser](browser.md).

## Reliable but early

We invest in simulation testing over certifications. Certifications come once the substrate is where we want it. See [reliability](reliability.md).

> [!IMPORTANT]
> ix is pre-GA: team of four, no SOC 2, two US regions, cross-tenant memory dedup. If any of these are disqualifying, see [what we don't claim yet](reliability.md#what-we-dont-claim-yet) before investing.
