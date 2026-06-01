# Hardware

Leased bare-metal hosts in two regions. No hyperscaler VM layer, no shared hypervisor, no networked block storage. Full software stack from kernel up. Rates in [pricing](pricing.md).

## Scaling capacity

Target is 2x current demand available at any time. When we need more:

- **Non-contested region or SKU.** New capacity lands in ~1 hour.
- **Contested region or tight SKU.** Up to ~4 weeks to rack new hosts.
- **Enterprise, dedicated bare metal.** Provisioned on your procurement timeline. See [pricing](pricing.md#enterprise).

## Regions

| Slug | Location |
|---|---|
| `us-west` | Hillsboro, Oregon |
| `us-east` | Vint Hill, Virginia |

> [!WARNING]
> US-only today. No EU or APAC regions. If you have data-residency obligations (GDPR) or latency-sensitive users outside North America, ix is not a fit today.

## Compute host

Every VM runs on a host with:

- **CPU.** AMD EPYC 9555. 64 cores / 128 threads. 3.2 GHz base, 4.4 GHz boost. Zen 5 "Turin".
- **RAM.** 512 GiB DDR5.
- **Storage.** 2× 1.92 TB + 6× 7.68 TB NVMe per host. Samsung PM1743-series (Gen5 enterprise NVMe, OEM part numbers `MZWL61T9HFLT` and `MZWL615THBLF`), attached directly to the EPYC's PCIe Gen5 root complex. No SAN, no NAS, no network-mounted block store between a VM and its disk.

## CPU benchmarks

- Single-thread: [PassMark](https://www.cpubenchmark.net/cpu.php?cpu=AMD+EPYC+9555&id=6455) (~3,757).
- Multi-thread: [SPECrate2017_int](https://www.spec.org/cpu2017/results/res2025q1/cpu2017-20250224-46544.html) (peak 1,610), or the same [PassMark](https://www.cpubenchmark.net/cpu.php?cpu=AMD+EPYC+9555&id=6455) page for a CPU-mark figure.
- Background: AMD [product page](https://www.amd.com/en/products/processors/server/epyc/9005-series/amd-epyc-9555.html), Phoronix [5th Gen EPYC review](https://www.phoronix.com/review/amd-epyc-9655).

## Storage performance

Direct-attached PCIe. No network hop between a VM's fsync and the drive.

Per 7.68 TB data drive peak ([PM1743 datasheet](https://semiconductor.samsung.com/ssd/enterprise-ssd/pm1743/)):

- Sequential read: up to 14,000 MB/s
- Sequential write: up to 6,600 MB/s
- Random 4K read: up to 2,500K IOPS
- Random 4K write: up to 250K IOPS

6× data drives form a ZFS raidz2 pool (4 data + 2 parity, ~30 TB usable). 2× system drives mirrored for host OS. Aggregate raidz2 ceiling:

- **Sequential read.** ~84 GB/s
- **Sequential write.** ~26 GB/s (4-wide data stripe)
- **Random 4K read.** ~15M IOPS
- **Random 4K write.** Hundreds of thousands of IOPS (raidz2 read-modify-write overhead)

Actual VM throughput depends on config, ZFS recordsize, and host sharing. No published fio runs yet.

Third-party PM1743 reviews: [StorageReview](https://www.storagereview.com/review/samsung-pm1743-ssd-review), [TweakTown 7.68 TB](https://www.tweaktown.com/reviews/10923/samsung-pm1743-e3-7-68tb-enterprise-ssd-og-gen5-standard-bearer/index.html), [TweakTown 15.36 TB](https://www.tweaktown.com/reviews/10954/samsung-pm1743-e3-15-36tb-enterprise-ssd-high-capacity-standard-bearer/index.html).
