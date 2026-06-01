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

## Balance and top-ups

Prepaid. Add funds, usage debits the balance. No invoices, no monthly minimum.

| Setting | Value |
|---|---|
| Minimum top-up | $5 |
| Maximum top-up | $1,000 per transaction |
| Presets | $10, $25, $50, $100 |

Top-ups go through Stripe Checkout. Successful charges credit the balance immediately on the webhook. Failed or canceled charges never credit. Saving the payment method is opt-in at checkout and only needed if you want auto-recharge.

```bash
ix billing top-up --amount 25 --open
```

## Auto-recharge

Optional. Set a threshold and a recharge amount. When the balance falls below the threshold, ix charges the saved payment method for the recharge amount.

| Field | Min | Max | Default |
|---|---|---|---|
| Threshold | $1 | $500 | $5 |
| Recharge amount | $5 | $1,000 | $25 |

A failed charge (declined card, expired card, anything Stripe surfaces) is recorded with a reason and pauses further auto-recharge attempts until the card is fixed or you top up by hand. The balance keeps falling in the meantime; the lifecycle below takes over once it hits zero.

## When the balance hits zero

Four phases. The clocks key off the moment the balance first reaches zero or below.

| Phase | Trigger | Duration | Effect |
|---|---|---|---|
| Active | balance > 0 | - | Normal use. |
| Compute grace | balance ≤ 0 | 24 hours | No new paid actions (start VM, fork, snapshot). Already-running VMs keep running and keep accruing charges. |
| Data retention | grace expired, balance still ≤ 0 | 30 days | VMs stopped. Snapshots, forks, and disk persist and are still billed. |
| Deleted | retention expired, balance still ≤ 0 | - | Stored data becomes eligible for deletion. |

Topping up before the deleted phase puts the account back to Active and clears the timers. The grace and retention values are product policy, not contract terms. If you need different SLA windows, ask on Slack ([access](access.md#support)).

## Usage visibility

Per-resource and per-VM, in near real time:

```bash
ix billing status            # balance, lifecycle phase, pending top-ups, auto-recharge
ix billing usage --since 30d # totals by resource, daily spend, recent events
```

The same data is exposed over the SDK. A streaming RPC (`subscribe_billing_alerts`) pushes balance and lifecycle changes if you want to react programmatically (kill long-running forks below a balance threshold, page on entering compute grace, etc).

## Enterprise

If ix will be a meaningful part of your stack, we can deploy bare metal into your datacenter (or colo next to your existing footprint) and run ix on it. Pricing moves from per-second utilization to a fixed monthly, sized to your fleet. Custom SLAs and region layouts are on the table. Email [andrew@ix.dev](mailto:andrew@ix.dev).
