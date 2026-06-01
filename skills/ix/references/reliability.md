# Reliability

ix is early. Read [What we don't claim yet](#what-we-dont-claim-yet) before committing production load.

## Simulation testing

Correctness-critical paths run under a deterministic simulator with fault injection: network partitions, disk errors, clock skew, process crashes, concurrent fork races. Failures replay exactly.

Surfaces under continuous test:

- Custom filesystem (copy-on-write, block dedup, fork isolation)
- Snapshot and restore (memory + disk consistency)
- VM lifecycle and scheduler

## Storage: VCFS

Custom filesystem underneath forks, snapshots, and tiering.

- **Content-addressed dedup across VMs.** Identical blocks stored once regardless of source (fork, OCI layer, same package across tenants). Forking a 100 GiB VM costs only divergent bytes. Snapshots are references into the same store. See [pricing](pricing.md).
- **Local durability.** Writes hit a ZFS raidz2 pool on the host's directly-attached NVMe (4 data + 2 parity, 6 drives, [hardware](hardware.md#storage-performance)). `fsync` returns after data is committed locally. Two parity disks tolerate any two drives in a host failing without data loss.

Designed for fsync-heavy workloads (Postgres, MySQL, SQLite WAL). Writes hit local NVMe; dedup runs out of band. No published p50/p99 fsync numbers yet, no Postgres TPC-B comparisons in-VM vs bare metal. If database performance matters, run your workload on both and compare. We owe these numbers; tracked in [roadmap](roadmap.md).

> [!WARNING]
> Today, snapshot durability is bounded by a single host. Snapshots are ZFS CoW snapshots on the same pool as the live VM. If the host is destroyed (fire, multi-disk failure beyond raidz2 tolerance), the VM and its snapshots go with it. Cross-host and cross-zone snapshot replication is on the roadmap but is not yet implemented. If durability beyond a single host matters today, export snapshots as OCI images and store them in your own registry.

## Host failure and recovery

Honest current state:

- **No live migration on host failure.** Live-migration infrastructure exists in the codebase for planned moves, not for unplanned host loss.
- **No automatic restart on a healthy host.** If a host goes down, its VMs are unavailable until the host comes back or you restore from a snapshot you exported off the host.
- **Snapshots are local to the host.** See the warning above.
- **Recovery path today.** For workloads where a host loss is unacceptable, run replicas across both regions ([network](network.md#inter-region)) and export snapshots periodically. Enterprise customers on dedicated bare metal can negotiate a custom replication topology.

Cross-host snapshot replication, automatic VM evacuation, and a published RPO/RTO are the next reliability milestone. Track in the [roadmap](roadmap.md).

## Security isolation

Each VM runs under `ix-vmm`, built on rust-vmm/KVM. Firecracker-like: small attack surface, minimal device model, one VM per host process. Network isolation is per-VM virtio-net on TAP. VMs don't see each other's traffic unless explicitly grouped ([network](network.md)).

> [!WARNING]
> KSM (cross-tenant memory page merging) is on by default. Timing side channels can in principle reveal whether a specific page exists in a neighbor. If you run sensitive key material, we can pin you to a KSM-disabled host or dedicated bare metal.

## Source code and audits

Honest current state:

- **Source code.** Closed source. Enterprise customers on dedicated bare metal can negotiate source access under NDA.
- **Independent audits.** None completed. No external pentest of the vmm or VCFS has been done yet. Internal audit-log infrastructure exists for operational events but is unrelated to third-party review.
- **Compliance certifications.** SOC 2 Type 1 and Type 2, HIPAA, ISO 27001 are not yet in process. Tracked in the [roadmap](roadmap.md).

If a security review is a hard requirement before you can adopt ix, email [andrew@ix.dev](mailto:andrew@ix.dev). We're scheduling an external pentest and can prioritize evaluators.

## Business continuity

- **Acquisition.** Keep the service running; honor active contracts. Escrow a migration window if the acquirer won't continue the product.
- **Wind-down.** Open-source the VMM and filesystem for self-hosting.
- **Portability.** Snapshots export as OCI images at any time.

> [!NOTE]
> These are intentions, not contract terms. If you need them in writing, ask on Slack. Enterprise customers on dedicated bare metal get source access and self-host runbooks.

## Support

Goal: 24/7 human coverage. Currently close. Slack response usually minutes during working hours. Service status is published at [status.ix.dev](https://status.ix.dev/). See [access](access.md#support).

## What we don't claim yet

> [!WARNING]
> These are things other mature providers offer that we don't yet. Read this list before putting production load on ix.
>
> - Five-nines availability.
> - Multi-region HA (and no EU / APAC regions at all today).
> - Cross-host snapshot replication and automatic VM evacuation on host failure.
> - Formal certifications (SOC 2, HIPAA, ISO 27001). Process starting soon.
> - External security audit / pentest of the vmm and VCFS.
> - Self-serve signup. Today onboarding is manual; see [access](access.md).
> - Public availability / durability SLAs with credits. Not yet default.

Why we invest in the substrate before the certifications: [philosophy](philosophy.md#reliable-but-early).

## SLAs on request

No default SLA yet. If you need contractual uptime, durability, MTTR, or data-residency terms, email [andrew@ix.dev](mailto:andrew@ix.dev). We'll scope something signed with credits. Enterprise/dedicated customers get this at onboarding.
