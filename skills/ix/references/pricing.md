# Pricing

Per-second billing on actual resource use. No reserved-capacity charges.

> [!NOTE]
> Leased bare metal with no hyperscaler underneath. Details in [hardware](hardware.md).

## Rates

| Resource | Rate | ≈ monthly at 100% |
|---|---|---|
| vCPU | $0.000008/s | $20 |
| GiB RAM | $0.000003/s | $8 |
| TiB disk | $0.000002/s | $5 |

## How it works

Allocation is a ceiling. Billing tracks utilization. A 64-vCPU VM at 1% CPU costs the same as a 1-vCPU VM at 64%.

## What gets measured

| Resource | Counter | Notes |
|---|---|---|
| CPU | cgroup v2 `cpu.stat usage_usec` | The VM's cgroup CPU time. Includes guest execution plus host-side overhead attributed to that cgroup (vmexits, KVM emulation). Converted to vCPU-seconds: `delta_usec / 1e6`. |
| RAM | cgroup v2 `memory.current` | Resident bytes attributed to the VM's cgroup (anon + file cache + kernel slab). Midpoint-averaged between consecutive samples, then converted to GiB-seconds. |
| Disk | post-dedup unique bytes attributed to the VM | After block-level dedup. Forks pay only for blocks that diverge from the parent. |

Sampling cadence is 60 seconds. Rates are quoted per second; the bill is the integral over the interval. KSM (cross-tenant page merging) is applied at the host level but does not reduce the cgroup `memory.current` reading, so KSM does not affect what you pay.

## Example

4 vCPU, 8 GiB RAM, 100 GiB disk, running at 20% CPU / 80% memory:

```
vCPU:   $0.000008/s x 0.8  = $0.000006/s
RAM:    $0.000003/s x 6.4  = $0.000019/s
disk:   $0.000002/s x 0.1  = $0.000000/s
                           ────────────
                             $0.000025/s  (~$65/mo)
```

## VM states and what they bill

| State | CPU | RAM | Disk |
|---|---|---|---|
| Running | metered | metered | metered |
| Sleeping / suspended (memory retained on host) | not metered | metered (memory still resident) | metered |
| Stopped (memory dropped, disk kept) | not metered | not metered | metered |
| Snapshot only (no live VM) | not metered | not metered | metered (only blocks unique to the snapshot) |

No per-VM, per-fork, or per-snapshot minimum charges. No reservation fees. The only sub-second-rounding floor in the system is a 1 MB threshold on disk and egress accounting to avoid noise on micro-deltas.

## Storage and forks

Block-level deduplication. Forking a 100 GiB VM does not cost 100 GiB. You pay for blocks that differ from the parent.

- Snapshots: free. They are the dedup mechanism.
- Forks: copy-on-write. Pay for divergent blocks only. No fork creation fee.

## Network

IPv4 + IPv6, up to 25 Gbps per VM, no per-byte fees, no ingress or egress charges.

## Fair use

No quotas, throttles, or rate limits. Billing is pure utilization, so heavy use earns revenue rather than costing us margin. A 64-vCPU VM pinned at 100% for weeks, sustained multi-Gbps egress, or ten thousand idle forks that burst occasionally are all fine. You pay for what you use; we have no reason to cap it.

## Enterprise

If ix will be a meaningful part of your stack, we can deploy bare metal into your datacenter (or colo next to your existing footprint) and run ix on it. Pricing moves from per-second utilization to a fixed monthly, sized to your fleet. Custom SLAs and region layouts are on the table. Email [andrew@ix.dev](mailto:andrew@ix.dev).
