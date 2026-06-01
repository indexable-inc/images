# Roadmap and owed numbers

What ix owes these docs and its users. An honest backlog beats a polished claim.
If something here is blocking you, say so on Slack ([access](access.md#support)); a
named user moves an item up.

## Numbers we should publish (and haven't)

- [ ] Rolling 30 / 90 / 365-day availability per region, from our own monitoring, linked from [reliability](reliability.md).
- [ ] Boot-latency distribution by image size (cached and uncached), beyond a single median.
- [ ] Fork creation p50 / p99 and first-write latency after fork. (Tracked: ENG-1051.)
- [ ] Snapshot create / restore times across sizes.
- [ ] Fsync p50 / p99 / p999 on VCFS under load, compared to direct NVMe on the same host. (Tracked: ENG-1052.)
- [ ] Postgres TPC-B-style numbers in-VM vs on bare metal. We make the claim; we owe the data. (Tracked: ENG-1052.)
- [ ] Network latency numbers: inter-region RTT, intra-region VM-to-VM, VM-to-internet from each region. (Inter-region bandwidth is 50 Gbps, documented in [network](network.md#inter-region); latency and VM-level throughput measurements still owed.)
- [ ] Storage durability track record (bytes written, scrub results, any data loss events: expected answer zero, but publish it).
- [ ] Published RPO / RTO once cross-host snapshot replication ships.

## Product gaps called out in other pages

- [ ] Reference OCI image for desktop / GUI agents, plus SDK helper for frame streaming ([desktop-gui](desktop-gui.md)).
- [ ] Streaming reads of long-running `exec` output in the Python SDK ([sdk/python](sdk/python.md)).
- [ ] Typed exception hierarchy in both SDKs (today: single `IxError`).
- [ ] Higher-level `CodeInterpreter`-style helper with structured output capture (TS).
- [ ] First-party tool wrappers for Vercel AI SDK, LangChain, Mastra.
- [ ] Scoped / short-TTL token minting and the device-attestation flow ([access](access.md), [browser](browser.md)).
- [ ] Fallback browser transport (WebSocket / WebRTC) for pre-18.2 Safari ([browser](browser.md)).
- [ ] Shell completions and tail / watch niceties in the CLI ([cli](cli.md)).
- [ ] First-class security-groups surface, VPC peering across groups, stable egress IPs, BYO BGP ([network](network.md)).

## Platform / company work

- [ ] Cross-host snapshot replication so VMs survive a host loss. (Tracked: ENG-1049.)
- [ ] Auto-restart VMs on a healthy host after host failure. (Tracked: ENG-1050.)
- [ ] First external pentest of vmm + VCFS. (Tracked: ENG-1055.)
- [ ] SOC 2 Type 1, then Type 2. HIPAA. ISO 27001. Timeline published once we're in audit.
- [ ] Published default availability and durability SLA with credits. Contractual SLAs available on request today ([reliability](reliability.md#slas-on-request)).
- [ ] EU region. APAC region. Neither committed yet; tell us which unblocks you.
- [ ] Self-serve signup gated by device attestation ([access](access.md#coming-soon)).
- [ ] 24/7 human coverage guarantee (currently a goal).
- [ ] Formalized wind-down open-source commitment in contract language ([reliability](reliability.md#business-continuity)).
- [ ] Individual pages for the three engineers beyond Andrew.

## Honest uncertainties

- [ ] KSM (cross-tenant memory page merging): we should decide whether to default it off and take the RAM cost, or keep it on and make opt-out trivial. Currently on; warning in [reliability](reliability.md#security-isolation).
- [ ] Per-tenant encryption key domains without giving up cross-VM dedup: unsolved in the general case. Enterprise-on-dedicated-metal sidesteps it; we don't yet have a multi-tenant story.
