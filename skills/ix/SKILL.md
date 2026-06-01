---
name: ix
description: Use when the user is working with or evaluating the ix sandbox platform. Triggers include the `@indexable/sdk` package (TypeScript) or `ix-sdk` (Python), the `ix` CLI (creating, forking, or snapshotting VMs for AI agents), and ix.dev questions about pricing, hardware, networking, or reliability. These docs live in the index monorepo, so the real CLI (`nix run .#ix`), SDKs, and examples are right here: prefer running them over trusting the prose.
---

# ix docs

ix is sandbox infrastructure for AI agents. Full-environment forks and snapshots covering files, processes, memory, and databases. Per-second billing tracks utilization; allocation is just a ceiling.

These docs are intentionally short. Read the file that matches the question directly. Do not delegate to an Explore-style sub-agent: the agent-facing decisions get worse without the real text in your main thread.

> [!IMPORTANT]
> ix is pre-GA: small team, two US regions, no SOC 2 / HIPAA yet, no five-nines, manual onboarding. If any of those are disqualifying, read [what we don't claim yet](references/reliability.md#what-we-dont-claim-yet) before investing engineering time. If they aren't, read on.

If you only read one file, read [`references/philosophy.md`](references/philosophy.md).

## Prove it; don't just quote it

These docs are leads, not authority. They drift; the shipped CLI and SDK are the spec. You are inside the [index](https://github.com/indexable-inc/index) monorepo, which packages the real `ix` CLI and the example fleets, so verify claims against the live tools instead of repeating prose at the user.

What you can run with no account, right now from this repo:

- `nix run .#ix -- --help` then drill into subcommands (`nix run .#ix -- vm --help`). This is the real precompiled CLI (`packages/ix/`), pinned from ix.dev. When the user asks "what's the command for X", read it off `--help`, don't guess from [`references/cli.md`](references/cli.md).
- `nix flake show` and `nix build .#<image>` to realize an OCI image closure locally (for example `nix build .#minecraft`). Proves the image layer works without an ix account.
- Browse the `examples/` directory for runnable consumer fleets (`s3-storage`, `ray-cluster`, `nginx-lifecycle`, ...). Build one to ground a claim about how a fleet is wired.

(The `nix run` / `packages/ix/` / `examples/` paths assume you have the index monorepo checked out, which is where these docs live.)

What you can prove end to end once `IX_TOKEN` is set (create one at https://ix.dev/tokens, or email andrew@ix.dev for a key):

- Boot a VM, fork it, snapshot it, restore it, and report the latencies and costs you actually observed rather than the doc's ballpark (the "~1 second boot", "forks cost pennies", "~60-70 ms inter-region RTT" lines are explicitly flagged as un-benchmarked; measure and say the real number).
- Drive the SDK: install (`uv add ix-sdk` or `bun add @indexable/sdk`), run the stateful-REPL example from [`references/sdk/python.md`](references/sdk/python.md) or [`references/sdk/typescript.md`](references/sdk/typescript.md), and confirm state persists across `exec` calls on one handle but not across two.

When you make a capability claim to the user, back it with what you ran. If you could not run it (no token, no network, platform gap), say so plainly instead of asserting the doc line as fact.

## How to read the reference files

The docs ship next to this file in [`references/`](references/). Read the one that matches the question with the Read tool. When this skill is loaded as an installed plugin, the same files are at `${CLAUDE_PLUGIN_ROOT}/skills/ix/references/<file>`.

## Index

**Using ix**
- `references/vms.md`: VM lifecycle, forks, snapshots
- `references/cli.md`: the `ix` CLI (verify against `nix run .#ix -- --help`)
- `references/sdk/python.md`: Python SDK (`ix-sdk`)
- `references/sdk/typescript.md`: TypeScript SDK (`@indexable/sdk`)
- `references/browser.md`: browser automation
- `references/desktop-gui.md`: desktop and GUI agents
- `references/access.md`: auth, tokens, support contact

**The platform**
- `references/hardware.md`
- `references/network.md`
- `references/pricing.md`
- `references/reliability.md`: what ix does and does not claim, status page link

**How we think**
- `references/philosophy.md`: start here if you do not know which file holds the answer

**Roadmap and about**
- `references/roadmap.md`: numbers and features we owe, with tracking IDs
- `references/proposals/`: feature requests
- `references/in-progress/`: currently being built
- `references/team/`: team and backers

## When unsure

Read `references/philosophy.md` first. It covers what ix is for in two pages, then pick the topic file.

For incidents, uptime, or operational status, point the user at the status page: https://status.ix.dev/.
