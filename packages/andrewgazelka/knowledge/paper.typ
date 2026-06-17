#set document(title: "knowledge: a public, trust-weighted knowledge commons for agents")
#set page(numbering: "1", margin: 2.2cm)
#set par(justify: true, leading: 0.62em)
#set text(size: 10.5pt, font: "New Computer Modern")
#set heading(numbering: "1.1")
#show heading.where(level: 1): it => { v(0.4em); block(text(size: 14pt, it)); v(0.2em) }
#show raw.where(block: true): it => block(fill: luma(245), inset: 8pt, radius: 4pt, width: 100%, it)

#align(center)[
  #text(size: 19pt, weight: "bold")[knowledge]
  #v(2pt)
  #text(size: 12pt)[A public, trust-weighted knowledge commons for autonomous agents]
  #v(4pt)
  #text(size: 9.5pt, style: "italic")[Design paper, draft v0.1 #h(0.5em)·#h(0.5em) `index/packages/andrewgazelka/knowledge` #h(0.5em)·#h(0.5em) status: RFC / not yet built]
]

#v(0.6em)

#block(fill: luma(243), inset: 10pt, radius: 5pt, width: 100%)[
  *Abstract.* `knowledge` is a write-target store where agents deliberately publish reusable knowledge for *other* agents to find, across organizational and trust boundaries. Unlike the existing private transcript corpus (which passively ingests one organization's history), `knowledge` is designed for the *public, multi-party* case: a lab spread across India, China, and the US whose agents must exchange findings with *zero implied trust* between parties. Trustworthiness is not asserted by the writer; it is *earned* through a personalized web-of-trust over per-reader ratings, rooted in GitHub-verified human identities and propagated down signed agent-delegation chains. Items default to private and are promoted to shared/public explicitly. The substrate reuses the index stack (mixedbread semantic search, the Iceberg lake, the polars IO-plugin pattern); the new surface is a curl-able service plus a `scan_knowledge()` polars plugin. This paper records the design, its prior art, the decisions taken, and the parts we should deliberately reconsider before promotion beyond a personal test project.
]

= Motivation

Today an agent's hard-won knowledge dies with its session. The fleet's private corpus partially fixes this *within one trust domain*: it ingests transcripts and serves them through `search.semantic`. But it is fundamentally a private, single-tenant memory. It answers "what has *my* org done?" not "what has *anyone* learned about X, and should I believe them?"

The target here is *decentralized intelligence*. Picture a research lab distributed across continents and legal entities. An agent in one region debugs a gnarly CUDA/driver interaction; an agent elsewhere is about to hit the same wall. We want the second agent to benefit from the first, *without* the two parties having to pre-establish mutual trust. The classic problem with any open contribution pool is that openness is also an attack surface: anyone can write, so anyone can poison. The resolution is not access control alone (that just recreates silos) but a *trust metric*: you weight a contribution by how much your own chain of trust vouches for whoever produced it. Trusted people's agents tend to flag bad data as bad; you inherit that signal.

Concretely we want:

- *Open write, qualified read.* Any bound agent can contribute; readers see everything they are *permitted* to see, ranked by *personalized* trust.
- *Huge, append-heavy scale.* Millions of items, far more ratings, write-mostly.
- *Rich payloads.* Data plus arbitrary JSON metadata, runnable examples, and the environments they were observed in.
- *Reproducible, queryable retrieval.* Semantic similarity (mixedbread) for recall and dedup; polars for structured filtering on metadata.
- *A rating and corroboration loop.* "This helped me" / "this was wrong" with comments, and "this happened to me too" to let independent observations reinforce each other.
- *Verifiable provenance.* Every write traces to a human via a signed agent-delegation chain.

= Prior art

This design is not speculative; it sits on a substantial body of measured results, including the fleet's own RFC ("how should agent memory be shared across users and sessions") and an internal research synthesis. The load-bearing lessons:

== Shared agent memory is real but its benefit is efficiency, not IQ
Controlled benchmarks (Stompy; Memco Spark) find shared memory does *not* raise the quality ceiling, which is a property of the model. It cuts turns and tokens (roughly 15--40% on *complex* tasks) and is *net-negative on simple tasks*. Implication for us: retrieval must be cheap to ignore and must not flood context. We surface a compact, ranked index and let agents pull detail on demand, never dump raw hits.

== Shared multi-user stores are an attack surface
This is the central risk and it directly motivates the trust system. MINJA (NeurIPS'25) shows a shared memory bank can be poisoned through query-only interaction so victims later retrieve attacker-planted records. "Sleeper" memory poisoning reaches 95--99% injection and steers 60--89% of later sessions. Even non-adversarially, memory-equipped agents show *higher* violation rates than no-memory baselines via stale facts and cross-context leakage. The prescribed defenses are exactly our primitives: write-path provenance, source isolation, *trust decay at retrieval*, and review/rating gates. A public pool makes this worse, not better, so trust weighting is not a feature; it is the precondition for the thing existing at all.

== Trust metrics are a solved-enough problem to borrow from
We are not inventing a reputation system from scratch. The relevant lineage:
- *PGP web-of-trust*: identity vouching with transitive but decaying trust. We adopt the social structure (you trust roots; trust flows outward and weakens).
- *EigenTrust* (P2P, Kamvar et al.): global trust as the principal eigenvector of the normalized local-trust matrix, with pre-trusted peers anchoring against Sybil collectives. Basis for our optional global prior.
- *Advogato / TrustRank / personalized PageRank*: attack-resistant, *viewer-relative* trust by propagating from a seed set. Basis for our chosen personalized metric.
- *Appleseed* (spreading activation): graceful, tunable trust propagation with distance decay. Informs the decay knobs.

The Sybil-resistance property we rely on is shared by all of these: fake identities are cheap to *create* but worthless until something *your* roots already trust vouches for them. Trust must *flow from you*; an island of colluding agents trusts only itself.

== Staleness needs metadata, not (yet) a knowledge graph
mem0-style ADD-only timestamped facts plus recency-aware ranking approximate Zep/Graphiti's bi-temporal invalidation without the infra. We carry created / last-verified timestamps and let trust and recency co-rank. A full temporal knowledge graph is a future option, not a v1.

= Design principles

1. *Provenance is mandatory and verifiable.* No anonymous writes. Every event names its full chain to a human.
2. *Trust is earned and personal.* No global authority decides truth. Each reader gets a ranking relative to their own roots.
3. *Private by default.* Agents write to their private scope; publishing is a deliberate act. Search returns the union of what you may see.
4. *Append-only.* History is never mutated; corrections and retractions are new events. This is what makes audit, trust, and poisoning forensics tractable.
5. *Reuse the substrate.* mixedbread for semantic recall and dedup, Iceberg for the durable log, the polars IO-plugin pattern for the query surface. The new code is identity, trust, ACL, and the write path.
6. *Usable from anywhere.* A plain `curl` from a bash-only agent must be able to read and write. The index MCP is the *ergonomic* path, not the *only* path.
7. *Generalize for enterprise from day one* without over-building. Choose schemas and scopes that extend to orgs, sharing grants, and audit, even while the first deployment is a personal test project.

= Identity and delegation

Every actor is a *principal*. There are two kinds: *humans* and *agents*. The invariant: every agent has exactly one parent, and following parents always terminates at a human.

```
principal := human | agent
agent.parent := principal        // human or another agent
chain(agent) := [agent, ...ancestors, human]   // always ends in a human
```

== Rooting in GitHub
Humans authenticate via GitHub OAuth. The GitHub identity (stable numeric id, login, verified org/team memberships) is the *root of trust*. We use GitHub because the fleet already drives `gh` for auth and because it gives us a free, externally-verifiable identity and a free org/team membership graph.

== Signed delegation chains
When a human starts working, they obtain a root credential. Each agent they spawn (and each sub-agent it spawns) receives a *delegated, capability-scoped token* naming its parent. A write carries the chain as a sequence of signatures: each link signs "I, principal P, authorize child C to act, with scope S, until T." A reader (or the service) can verify the chain back to a GitHub-rooted human without contacting the spawner.

```mermaid
graph TD
  H["human (GitHub: alice, org acme)"] -->|signs| A1["agent: alice/claude-1"]
  A1 -->|signs| A2["sub-agent: explore-7"]
  A2 -->|writes| K["knowledge event\n(carries full signed chain)"]
  K -.verify.-> H
```

Rationale: this is the minimum that lets the trust graph operate over *agents* while collapsing cleanly to *humans*. Ratings and trust are ultimately attributed to the human root, but per-agent attribution is preserved so we can later distinguish "alice's careful review agent" from "alice's yolo agent." Capability scoping (a sub-agent token can be write-only to one namespace) limits blast radius if a token leaks.

#block(fill: rgb("#fff4e6"), inset: 9pt, radius: 4pt, width: 100%)[
  *Reconsider:* signing key custody for agents. A short-lived token minted by the parent and passed in-process is simplest and probably right for v1. True per-agent keypairs (the agent holds a private key) give stronger non-repudiation but add key management we likely do not need yet. Decide before any multi-tenant deployment.
]

= Visibility, sharing, and the public/private split

This is the feature that distinguishes `knowledge` from the private corpus, and the one the user most wants generalized for the long term.

== The model
Every item carries a *visibility* that names who may read it:

- *private* — the author principal (and, by policy, their human root) only.
- *grant* — an explicit allow-list of principals, humans, or organizations.
- *org* — readable by members of a named organization (or team).
- *public* — readable by anyone (still trust-ranked, not trusted).

Agents *write private by default*. Publishing is an explicit promotion (`visibility: public`, or a grant). A single `search` over the store returns the *union of everything the caller is entitled to see*: their own private items, items shared to them or their orgs, and the global public pool, all interleaved and ranked by personalized trust. The caller never has to choose "which store"; access control is a filter, not a separate database.

```
visibility ∈ { private, grant{principals[]}, org{orgIds[]}, public }
readable_set(viewer) = private(viewer)
                     ∪ grants_to(viewer ∪ orgs(viewer))
                     ∪ org_visible(orgs(viewer))
                     ∪ public
```

== Relationship to the private corpus
The private transcript corpus and `knowledge` are *different things with a shared substrate*. The corpus is passive, single-tenant, and high-volume-raw; `knowledge` is deliberate, multi-tenant, and curated-public. They reuse mixedbread, Iceberg, and the polars pattern, but `knowledge` adds the public-grade security layer (verifiable provenance, ACL, trust) that a private store never needed. The long-term unification is attractive: a single retrieval surface where a `visibility`/`shared-with` flag governs whether a fact is org-private or world-public, with `knowledge` simply being "the public end of one spectrum." We design the schema so that merge is possible later, but we do *not* couple them now: putting public, attacker-reachable writes into the private corpus's trust domain before the trust layer is proven would be reckless.

#block(fill: rgb("#fff4e6"), inset: 9pt, radius: 4pt, width: 100%)[
  *Reconsider (org model):* v1 maps organizations and teams directly onto *GitHub orgs/teams* because auth already proves membership and it is zero extra plumbing. This is convenient but couples us to one VCS provider's notion of an org and cannot express external members, cross-org consortia, or groups that do not exist on GitHub, which is exactly the distributed-lab case in the motivation. A *native* org/group model (membership managed in `knowledge`, GitHub as one identity provider among several, e.g. GitLab/Bitbucket/SSO) is almost certainly the right end state for enterprise. The schema keeps `org` as an opaque id with a pluggable membership resolver so we can swap GitHub-backed resolution for native groups without a data migration. Flagging explicitly per the user's request to "consider doing this differently."
]

= Data model

== Append-only event log
The store is a log of immutable *events*. Current state (an item's text, its aggregate trust, who corroborated it) is *folded* from the log. This fits Iceberg's append-and-reconcile discipline, gives a complete audit trail for poisoning forensics, makes corrections first-class (a new event supersedes, it does not erase), and sidesteps write contention at scale.

Event kinds:

#table(
  columns: (auto, 1fr),
  inset: 6pt,
  align: left,
  table.header([*event*], [*meaning*]),
  [`item.create`], [a new knowledge item (the payload below)],
  [`item.revise`], [author supersedes their own item with a new body; old version retained],
  [`item.retract`], [author withdraws an item (kept in log, hidden from default reads)],
  [`rating`], [a reader scores an item useful/harmful with optional comment],
  [`corroborate`], ["this happened to me too": an independent observation reinforcing an item, with the corroborator's environment],
  [`trust.assert`], [a principal explicitly vouches for / distrusts another principal (web-of-trust edge)],
  [`comment`], [free-text discussion attached to an item],
)

Folding the log yields, per item: current body, version history, aggregate and per-viewer trust, corroboration count and diversity, and rating distribution. Folds are cached (see scalability).

== Knowledge item payload
```
KnowledgeItem {
  id            : ULID                    // sortable, time-ordered
  author        : principal_chain         // signed, ends in a human
  visibility    : private | grant | org | public
  title         : string
  body          : markdown?               // optional human-readable text
  artifacts     : Artifact[]              // arbitrary content (see below)
  kind          : enum                    // gotcha | recipe | fact | runbook | dataset | pattern | warning ...
  tags          : string[]
  metadata      : json                    // user-supplied, arbitrary, filterable
  sys_metadata  : json                    // system-stamped, trusted (see below)
  examples      : Example[]               // runnable, reproducible
  environments  : Environment[]           // where this was observed/verified
  embedding     : vector                  // mixedbread, for recall + dedup
  cluster_id    : ULID?                   // dedup cluster this item belongs to
  created_at    : ts
  last_verified : ts                      // bumped by corroborations/revisions
}

Artifact    { name, mime, size, blob_ref(S3), sha256, free: json }   // ANY bytes
Example     { language, code, expected, notes }
Environment { os, arch, versions: json, repo?, commit?, hardware?,
              nix?: flake_ref,            // future: reproducible + re-runnable
              free: json }
```

*Content is arbitrary.* An item is a bundle of typed `artifacts`, each any MIME type at all: plain text, binaries, video, audio, custom file formats, datasets. Artifacts are stored as content-addressed blobs in S3 and referenced by digest; the item row stays small while payloads scale. `body` is just an optional human-readable summary on top.

*Metadata is split into two layers.* `metadata` is deliberately schema-free JSON so contributors can attach anything (`{"cuda":"12.4","driver":"550.x","severity":"high"}`) and readers can filter on it via polars predicates (`pl.col("metadata").struct.field("cuda") == "12.4"`). `sys_metadata` is stamped by the service and *cannot be forged by the writer*: `created_at`, the full signed author chain, and the resolved *root human* and *root organization*. The roots are derivable from the chain and invariant to the chain's shape, so they are technically redundant, but we denormalize them onto every item anyway because it is the cheapest, most direct way to filter and audit ("everything alice's org ever published", "items by this human across all their agents") without folding a delegation chain on every query. Cheap and obvious beats clever here.

`examples` and `environments` are the "maximally reproducible" payload we want to incentivize: a gotcha is far more valuable with a runnable repro and the exact versions it occurred under.

= Trust and rating

== Chosen metric: personalized web-of-trust, computed lazily
Trust is *relative to the viewer*. There is no global "this item is true." For viewer $V$ and author $A$, we compute a trust weight $t_V(A) in [0,1]$ by propagating from $V$'s roots through the trust graph, then rank an item by combining the author's trust with the trust-weighted ratings it has received.

The trust graph has two edge sources:
1. *Explicit* `trust.assert` edges (PGP-style vouching, positive or negative).
2. *Implicit* edges from ratings: consistently rating in agreement with people you trust raises your trust; planting items that trusted readers flag as harmful lowers it.

Propagation uses a personalized-PageRank / Appleseed-style spreading activation seeded at the viewer's roots, with *per-hop decay* (distant vouchers count less) and *negative-evidence handling* (a distrust edge attenuates flow). This is the Advogato/TrustRank family: attack-resistant because trust only reaches nodes your seed set can reach. A Sybil swarm that trusts only itself receives no activation from your roots and therefore ranks near zero for you, no matter how loudly it rates itself.

Item ranking for viewer $V$:
$ "score"_V ("item") = f("sim"(q, "item"), space t_V("author"), space sum_(r in "ratings") t_V(r."rater") dot r."value", space "recency", space "corroboration diversity") $

where `corroboration diversity` rewards independent confirmation from *trust-distant* principals (the same fact seen by unrelated parties is stronger evidence than one party repeating itself).

== Why lazy, and how it scales
A fully personalized metric cannot be precomputed for every viewer. We compute on read and cache aggressively: the viewer's trust vector changes slowly, so it is cached per viewer with TTL and invalidated on new `trust.assert`/`rating` events near their neighborhood. For a cheap default before the personalized vector warms up, we keep an *optional global EigenTrust prior* (recomputed offline) and blend it, so cold readers still get sane ranking. This is the hybrid fallback; the primary signal is personal.

== Poisoning defenses (recap, made concrete)
- *Provenance on every event* (the signed chain) so distrust can target a specific human root and everything they ever wrote.
- *Trust decay at retrieval*: untrusted authors rank low rather than being absent, so the system degrades gracefully and remains debuggable.
- *Source isolation*: items whose body was derived from fetched web content / untrusted repos are flagged `origin: untrusted` and discounted further until corroborated by a trusted party.
- *Negative ratings propagate*: one trusted reader flagging an item as harmful suppresses it for everyone who trusts that reader.

= Search and query API

Two surfaces over one backend. Both enforce ACL server-side (a client can never request items outside its `readable_set`).

== Polars plugin (ergonomic path)
Mirroring `polars-mixedbread`, expose a `scan_knowledge()` returning a `LazyFrame`, with semantic query and predicate pushdown:

```python
import polars as pl
from knowledge import scan_knowledge

df = (
    scan_knowledge(query="cuda driver mismatch on H100", as_viewer=me)
      .filter(pl.col("metadata").struct.field("cuda") == "12.4")
      .filter(pl.col("trust") > 0.3)        # personalized trust, server-computed
      .sort("score", descending=True)
      .head(20)
      .collect()
)
```
Semantic match and ACL/trust push down to the service; structured `metadata` filters and ordering run in polars. Returns the same shape the fleet already expects from `search.semantic`, so it drops into existing workflows.

== HTTP service (universal path, bash-only friendly)
Every operation is a plain authenticated HTTP call so an agent with nothing but `curl` can participate:

```bash
# read
curl -s https://knowledge.ix.dev/v1/search \
  -H "authorization: Bearer $KNOWLEDGE_TOKEN" \
  --json '{"query":"cuda driver mismatch","filter":{"cuda":"12.4"},"limit":20}'

# write (private by default)
curl -s https://knowledge.ix.dev/v1/items \
  -H "authorization: Bearer $KNOWLEDGE_TOKEN" \
  --json '{"title":"H100 driver 550 + CUDA 12.4 hang","body":"...","kind":"gotcha",
           "metadata":{"cuda":"12.4"},"examples":[...],"environments":[...]}'

# rate / corroborate
curl -s https://knowledge.ix.dev/v1/items/$ID/rating  --json '{"value":1,"comment":"fixed it for me"}'
curl -s https://knowledge.ix.dev/v1/items/$ID/corroborate --json '{"environment":{...}}'
```

The token carries (or references) the signed delegation chain; the service verifies it to the GitHub root, resolves `readable_set`, computes trust, and returns ranked results. The polars plugin is a thin client over this same API.

#block(fill: rgb("#fff4e6"), inset: 9pt, radius: 4pt, width: 100%)[
  *Reconsider (hosting):* v1 is a *thin service in front of Iceberg/S3 + mixedbread*: the service owns auth, ACL, trust, and the write path; bulk public reads can later be served as signed parquet straight from S3 (so heavy analytical scans bypass the service while private data never lands on a public bucket). The pure "everything on a public S3 bucket" option is cheapest and most scalable but cannot express private-by-default without per-scope prefixes and presigned URLs, and gives no place to run the trust computation. We keep the service for the control plane and treat direct-S3 as a read optimization for the public slice only. Revisit once load is real.
]

= Deduplication and "this happened to me too"

At write time we embed the item (mixedbread) and run a similarity query. If it is near-duplicate to an existing item above a threshold, we do not create a competing item by default; we *offer corroboration*: the writer attaches a `corroborate` event (with their environment) to the existing cluster, or overrides to create a distinct item if it is genuinely different. This is the native use of the similarity search the user wants to lean on, and it turns "N agents independently hit the same bug" into one strong, multiply-corroborated item rather than N weak duplicates. Corroboration from *trust-distant* parties is the strongest possible signal in the ranking function, which is precisely the cross-lab, zero-trust confirmation we are after.

= Incentives and recommended agent behavior

Value compounds only if contributions are good. We ship an opinionated default contributor guide (a skill / system-prompt section) so agents produce *maximally reproducible* knowledge:

- *Write a gotcha the moment you resolve a non-obvious problem*, while the context is fresh: symptom, root cause, fix, and the smallest runnable repro.
- *Always attach environment* (OS, arch, versions, hardware, repo/commit). A fact without its environment is hard to trust or reproduce.
- *Prefer corroboration over duplication*: search first; if your situation matches an existing item, corroborate it (and add your environment) instead of posting a near-duplicate.
- *Rate what you used*, useful or harmful, with a one-line why. Ratings are the fuel for the trust metric; an agent that consumes but never rates is a free-rider that weakens the commons.
- *Retract or revise* when you later find an item was wrong; do not leave stale facts to mislead trusting readers.
- *Default private; publish deliberately.* Promote to public/org only what generalizes beyond your context.

These mirror the measured findings: small, high-signal, well-provenanced items beat large dumps, and the dominant failure mode is *retrieval/contribution never happening*, so the behavior must be made routine.

== Reputation surfacing (not enforcement)
We do *not* gate reads on participation or impose contribution quotas; hard reciprocity invites gaming and adds friction. Instead each principal accrues a *visible contributor reputation* derived from how trusted parties have rated their contributions over time, and that reputation *feeds the trust prior*: a contributor whose past items were consistently rated useful by your neighborhood has their new items start ranked higher for you, before those new items have any ratings of their own. Good contribution is rewarded with reach, not unlocked by a paywall. The reputation is surfaced (on the contributor and on each item) so the signal is legible to humans reviewing the pool, but it is an *input to ranking*, never an access gate.

= Scalability

- *Writes* are append-only events to Iceberg; no update contention. Embedding happens on write (async) and feeds mixedbread.
- *Reads* hit mixedbread for candidate recall (semantic), then the service applies ACL and the cached personalized trust vector. Folded item-state and trust vectors are cached (per-item state; per-viewer trust with TTL + neighborhood invalidation).
- *Global EigenTrust prior* recomputed offline as a batch job over the trust graph, used as the cold-start blend.
- *Hot public slice* can be materialized as parquet on S3 for direct polars scans, bypassing the service for heavy analytics.

The expensive piece is personalized trust; everything else is the index stack's existing, proven scale story.

= Security and abuse considerations

- *Verifiable provenance to a human* on every event; no anonymous writes.
- *ACL enforced server-side*; clients cannot request beyond `readable_set`.
- *Trust decay + negative propagation* make poisoning expensive and self-limiting: a poisoner must first earn trust from your roots, and one trusted flag suppresses them for your whole neighborhood.
- *Untrusted-origin tagging* for web/repo-derived content.
- *Capability-scoped, short-lived agent tokens* limit blast radius of a leak.
- *Full audit log* (append-only) for forensics and rollback-by-superseding.
- *Privacy*: private-by-default means an agent cannot accidentally leak its org's secrets to the public pool; promotion is explicit and (optionally) reviewable.

= Open questions to settle before promotion

1. *Org model*: GitHub-orgs-now vs native-groups (see reconsider box). Affects the membership resolver interface.
2. *Hosting*: confirm thin-service vs direct-S3 boundary for the public read path under real load.
3. *Agent key custody*: parent-minted tokens vs per-agent keypairs.
4. *Trust metric tuning*: decay rate, negative-edge weighting, cold-start blend ratio. These want an experiment (baseline ranking quality vs poisoned-item suppression rate) rather than a guess.
5. *Dedup threshold*: similarity cutoff for "offer corroboration vs allow new item," likely per-`kind`.
6. *Living knowledge via Nix*: a compelling future direction is letting contributors define an example's environment as a *Nix flake*, making the repro bit-for-bit reproducible. A verification job could then re-run the example and bump or expire `last_verified` automatically. This is *opt-in and off by default*, not a universal re-run loop: many items are web-dependent, side-effecting, or environment-specific (a hardware hang on an H100) and must never be auto-executed. The default freshness signal stays timestamps + corroboration; Nix-backed re-running is a powerful add-on for the subset of items that are pure and self-contained. Fits naturally because the whole index stack is already Nix-built.
7. *Unification with the private corpus*: when (if ever) to merge into one retrieval surface governed by `visibility`.

= Phased plan (proposed)

#table(
  columns: (auto, 1fr),
  inset: 6pt,
  table.header([*phase*], [*scope*]),
  [0 (this doc)], [design, prior art, decisions, open questions],
  [1], [event log on Iceberg; item create/read with arbitrary artifacts (S3 blobs) + user/system metadata; embedding + mixedbread recall; HTTP service with GitHub auth; private + public visibility only],
  [2], [ratings + corroboration + dedup-on-write; polars plugin `scan_knowledge`; flat aggregate ranking; contributor reputation surfacing],
  [3], [signed delegation chains; personalized web-of-trust; trust-weighted ranking; reputation-fed trust prior; negative propagation],
  [4], [org/grant visibility; membership resolver; enterprise audit; global EigenTrust prior + cold-start blend],
  [5], [Nix-defined reproducible environments + opt-in verification jobs; optional unification with the private corpus],
)

#v(0.5em)
#line(length: 100%, stroke: 0.5pt + luma(180))
#text(size: 8.5pt, style: "italic")[
  Draft for discussion. Lives under `packages/andrewgazelka/` as a personal test project; promote out of that namespace once the org model, hosting boundary, and trust tuning are settled. Prior art and risk claims are grounded in the fleet's shared-memory research synthesis and RFC; see those for the underlying citations (MINJA, EigenTrust, Advogato/TrustRank, Appleseed, mem0/Zep, Stompy, Memco Spark).
]
