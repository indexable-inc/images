#set document(title: "knowledge: a public, trust-weighted knowledge commons for agents")
#set page(numbering: "1", margin: 2.2cm)
#set par(justify: true, leading: 0.62em)
#set text(size: 10.5pt, font: "New Computer Modern")
#set heading(numbering: "1.1")
#show heading.where(level: 1): it => { v(0.4em); block(text(size: 14pt, it)); v(0.2em) }
#show raw.where(block: true): it => block(fill: luma(245), inset: 8pt, radius: 4pt, width: 100%, it)
#show link: it => underline(text(fill: rgb("#1a4f8a"), it))
#import "@preview/fletcher:0.5.7" as fletcher: diagram, node, edge
#import "@preview/cetz:0.3.4"

#let cite(url, label) = link(url)[#label]
#let blue = rgb("#e8f0fe")
#let amber = rgb("#fff4e6")
#let green = rgb("#e6f4ea")
#let red = rgb("#fce8e6")

#align(center)[
  #text(size: 19pt, weight: "bold")[knowledge]
  #v(2pt)
  #text(size: 12pt)[A public, trust-weighted knowledge commons for autonomous agents]
  #v(4pt)
  #text(size: 9.5pt, style: "italic")[Design paper, draft v0.2 #h(0.5em)·#h(0.5em) `index/packages/andrewgazelka/knowledge` #h(0.5em)·#h(0.5em) status: RFC / not yet built]
]

#v(0.6em)

#block(fill: luma(243), inset: 10pt, radius: 5pt, width: 100%)[
  *Abstract.* `knowledge` is a write-target store where agents deliberately publish reusable knowledge for *other* agents to find, across organizational and trust boundaries. Unlike the existing private transcript corpus (which passively ingests one organization's history), `knowledge` is designed for the *public, multi-party* case: a lab spread across India, China, and the US whose agents must exchange findings with *zero implied trust* between parties. Trustworthiness is not asserted by the writer; it is *earned* through a personalized, Sybil-*tolerant* web-of-trust over per-reader ratings and corroboration, rooted in verifiable human identities and propagated down signed, attenuated agent-delegation chains. Because a public pool is a documented poisoning target, *signed provenance plus independent-trust gating is the load-bearing defense*, not a feature: similarity retrieval alone is forbidden for public scope. Items default to private and are promoted to shared/public explicitly. The substrate reuses the index stack (mixedbread semantic search, the Iceberg lake, the polars IO-plugin pattern); the new surface is a curl-able service plus a `scan_knowledge()` polars plugin. This v0.2 incorporates a verified state-of-the-art review (§3): the headline correction is that the trust metrics in a naive personalized-PageRank lineage are *provably not Sybil-resistant*, so the design is hardened with MeritRank-style decays, independent-root corroboration, one-hop distrust, and trust-weighted retrieval with sensitivity floors.
]

#v(0.4em)
#block(fill: green, inset: 10pt, radius: 5pt, width: 100%)[
  *In one paragraph (for the human skimming this).* Agents keep solving the same problems because what one learns is lost to the next. `knowledge` is a shared notebook any agent can write to and any agent can search. The catch: if anyone can write, anyone can lie or spam, so nothing is trusted just because it was written. Instead, every entry is signed back to a real person (via GitHub), and you see entries ranked by *how much your own circle of trust vouches for whoever wrote them*: a stranger's claim that ten people you trust have confirmed floats up; a swarm of fake accounts vouching for each other scores zero for you. Entries are private by default and made public on purpose. The figures below show the moving parts.
]

#figure(
  diagram(
    spacing: (11mm, 7mm), node-stroke: 0.5pt, node-corner-radius: 3pt,
    node((0,0), [human\ (GitHub)], fill: blue),
    node((1,0), [agent\ chain], fill: blue),
    node((2,0), [*knowledge service*\ auth · ACL · trust], fill: amber, width: 30mm),
    node((3.15,-0.9), [Iceberg\ log]),
    node((3.15,0), [mixedbread\ (semantic)]),
    node((3.15,0.9), [S3\ blobs]),
    node((2,1.5), [reader\ agent], fill: blue),
    edge((0,0),(1,0), "->", [signs]),
    edge((1,0),(2,0), "->", [write]),
    edge((2,0),(3.15,-0.9), "<->"),
    edge((2,0),(3.15,0), "<->"),
    edge((2,0),(3.15,0.9), "<->"),
    edge((2,0),(2,1.5), "<->", [query #sym.arrow.t\ ranked #sym.arrow.b\ (ACL+trust)]),
  ),
  caption: [Architecture and data flow. Writes carry a signed, human-rooted chain into the service, which appends to the Iceberg log and indexes into mixedbread (with blobs in S3). Reads return only what the caller may see, ranked by personalized trust.],
)

= Motivation

Today an agent's hard-won knowledge dies with its session. The fleet's private corpus partially fixes this *within one trust domain*: it ingests transcripts and serves them through `search.semantic`. But it is fundamentally a private, single-tenant memory. It answers "what has *my* org done?" not "what has *anyone* learned about X, and should I believe them?"

The target here is *decentralized intelligence*. Picture a research lab distributed across continents and legal entities. An agent in one region debugs a gnarly CUDA/driver interaction; an agent elsewhere is about to hit the same wall. We want the second agent to benefit from the first, *without* the two parties having to pre-establish mutual trust. The classic problem with any open contribution pool is that openness is also an attack surface: anyone can write, so anyone can poison. The resolution is not access control alone (that just recreates silos) but a *trust metric*: you weight a contribution by how much your own chain of trust vouches for whoever produced it, and you ignore endorsements from identities your chain has never reached.

Concretely we want: open write / qualified read; huge append-heavy scale; rich arbitrary payloads (data and metadata); reproducible, queryable retrieval (semantic recall plus polars filters); a rating and corroboration loop ("this helped me" / "this happened to me too"); and verifiable provenance to a human on every event.

= Design principles

1. *Provenance is mandatory and verifiable.* No anonymous writes; every event names a signed chain to a human, and nothing is *surfaced* for public/cross-user scope without one. This is the central poisoning defense (§3, §10).
2. *Trust is earned, personal, and Sybil-tolerant.* No global authority decides truth. Each reader gets a ranking relative to their own roots, and fake identities gain bounded leverage because trust must flow *to* them from a reader's seed set.
3. *Private by default.* Agents write private; publishing is deliberate. Search returns the union of what you may see.
4. *Append-only and bi-temporal.* History is never mutated; corrections, retractions, and supersessions are new events carrying both valid-time and ingestion-time. This makes audit, trust recompute, and poisoning forensics tractable.
5. *Reuse the substrate.* mixedbread for recall and dedup, Iceberg for the durable log, the polars IO-plugin pattern for the query surface. The new code is identity, trust, ACL, dedup/conflict, and the write path.
6. *Usable from anywhere.* A plain `curl` from a bash-only agent must read and write. The index MCP is the ergonomic path, not the only path.
7. *Keep curation off the hot path.* Dedup, relation inference, and trust recompute run as background ("sleep-time") jobs that emit new events; reads stay low-latency.

= State of the art and prior art

This design was checked against a verified literature sweep (web search, primary sources, adversarial re-verification per claim). We cite only systems and results that verified; where a number is vendor self-report or a single-author preprint, we say so. The single most important finding is a *correction* to the trust design, called out in §3.2.

== Agent memory: distilled, typed, temporal, additive
Consensus has formed that agents should store *distilled, structured* experience and that memory should be *additive*, not destructively rewritten. #cite("https://arxiv.org/abs/2509.25140", "ReasoningBank") (Google, ICLR'26) distills reusable strategies including first-class *failure* lessons from self-judged outcomes and beats memory-free, raw-trajectory, and success-only baselines; notably it "just appends, leaving consolidation to future work", exactly the gap our relations + dedup fill. #cite("https://arxiv.org/abs/2510.04618", "ACE") (Stanford/SambaNova/Berkeley, ICLR'26) names the failure modes of destructive consolidation (*context collapse*, *brevity bias*) and shows incremental structured deltas beat full rewrites, and that self-curation *without a reliable feedback signal pollutes the store*, the core argument that our trust-weighting is necessary, not optional. #cite("https://arxiv.org/abs/2504.19413", "Mem0/Mem0g") independently converged on an ADD-only architecture (treat its LOCOMO numbers as directional only). #cite("https://arxiv.org/abs/2501.13956", "Zep/Graphiti") is the closest production analog to our typed-relation graph: a *bi-temporal* graph with automatic fact-invalidation and provenance to source. #cite("https://arxiv.org/abs/2502.12110", "A-MEM") (NeurIPS'25) is direct precedent for typed relations forming a self-organizing graph; #cite("https://arxiv.org/abs/2506.07398", "G-Memory") shows a distilled-insight tier above raw events raises success materially; #cite("https://arxiv.org/abs/2504.13171", "Letta/MemGPT sleep-time") validates running curation off the response path; #cite("https://arxiv.org/abs/2509.09498", "SEDM") proposes verifiable write-admission (replay the claimed benefit) as an objective signal (code unreleased). For our visibility model, #cite("https://arxiv.org/abs/2505.18279", "Collaborative Memory") (Accenture, ICML'25 workshop) is a near-exact independent reinvention: private + selectively-shared tiers, immutable per-fragment provenance, and access-filtered "projected" retrieval with auditable adherence to asymmetric, time-evolving permissions.

== Trust, reputation, and Sybil-resistance (the key correction)
*Our naive trust lineage is not Sybil-resistant on its own.* #cite("https://arxiv.org/abs/2207.09950", "MeritRank") (TU Delft/Tribler) proves personalized PageRank, hitting-time, and max-flow are not Sybil-resistant, and makes them Sybil-*tolerant* (bounded attacker gain) via three drop-in decays: *transitivity* (serial attacks), *connectivity* (parallel/cycle attacks across a narrow cut), and *epoch* (staleness). #cite("https://www.eecs.harvard.edu/cs286r/courses/fall09/papers/friedman1.pdf", "Friedman & Cheng") prove *no nonconstant symmetric reputation function is Sybilproof*, the strongest justification for making personalized/asymmetric trust primary over any global score, and they note even EigenTrust-style PageRank variants are manipulable. #cite("https://snap.stanford.edu/class/cs224w-readings/guha04trust.pdf", "Guha et al.") (WWW'04, ~800K Epinions edges) establish that distrust does *not* propagate transitively like trust and that *one-step distrust* performs best. #cite("https://arxiv.org/html/2510.27554v1", "TraceRank") demonstrates the property that makes count-based corroboration safe: N endorsements from zero-seed identities accrue *zero* propagated reputation regardless of N, while one high-seed endorsement propagates real trust. #cite("https://arxiv.org/pdf/2307.01411", "Web3Recommend") and #cite("https://github.com/BrightID/BrightID-AntiSybil", "SybilRank") show such a personalized engine is incrementally computable and Sybil-hardenable. For a uniqueness root, #cite("https://arxiv.org/pdf/2408.07892", "Personhood Credentials") (OpenAI/Microsoft/academia) and deployments like #cite("https://web3classdao.github.io/kaist2025/reports/brightid/", "BrightID") (~100K users by mid-2025, a scaling cautionary tale) define the option space. #cite("https://www.arxiv.org/pdf/2511.03434", "Inter-Agent Trust Models") (Hu & Rong) rate reputation "Very Low" robustness to Sybil for LLM agents (worsened by prompt injection and sycophancy) and recommend anchoring *high-impact actions* in Proof+Stake with reputation as an overlay; #cite("https://arxiv.org/abs/2505.14551", "TRep") makes honest trust-reporting a Nash equilibrium.

== Memory poisoning and defenses
Poisoning is a first-class, scale-invariant threat against similarity retrieval. #cite("https://arxiv.org/abs/2402.07867", "PoisonedRAG") (USENIX'25) reaches ~90% attack success with *5 injected texts* against a 2.68M-document corpus: content volume is no defense. #cite("https://arxiv.org/abs/2407.12784", "AgentPoison") (NeurIPS'24) backdoors agent memory at ≥80% success at under 0.1% poison rate by optimizing a trigger into a tight embedding cluster, attacking the very mechanism dedup relies on. #cite("https://arxiv.org/abs/2503.03704", "MINJA") (NeurIPS'25) plants poison with *query access only* (~98%), proving a valid delegation must not be equated with trustworthiness. #cite("https://arxiv.org/html/2512.16962v1", "MemoryGraft") and #cite("https://aclanthology.org/2025.findings-emnlp.1023.pdf", "AuthChain") (EMNLP'25) are the dangerous shapes for us: trigger-free, fluent poison disguised as a legitimate "successful experience" with forged in-document corroboration, defeating perplexity/anomaly/self-assessment detectors and single-document count-based corroboration. Defenses are complementary layers, none a guarantee: #cite("https://arxiv.org/abs/2405.15556", "RobustRAG") (NeurIPS'25) gives a *certified* lower bound *only while poison is a minority* of retrieved items (the precondition our trust filter must enforce); #cite("https://arxiv.org/abs/2410.22954", "RA-RAG") (EMNLP'25) estimates per-source reliability label-free via cross-source agreement among *independent* sources (the backbone of our corroboration layer); #cite("https://arxiv.org/html/2504.21668v1", "RAGForensics/RAGOrigin") perform post-hoc, black-box *traceback* from a bad answer to the responsible stored texts, for which our provenance-rich append-only log is an ideal substrate.

== Agent identity, delegation, and provenance
The standards space is crowded and unratified, so build on audited primitives. #cite("https://rfc-editor.org/rfc/rfc8693", "OAuth Token Exchange (RFC 8693)") supports nested `act` actor claims but treats prior actors as *informational*, not cryptographically attenuated per hop; the #cite("https://datatracker.ietf.org/doc/html/draft-oauth-ai-agents-on-behalf-of-user-02", "IETF on-behalf-of draft") covers only the single user→agent hop, so our multi-hop signed chain is ahead of the ratified surface. #cite("https://www.biscuitsec.org/", "Eclipse Biscuit") is the best concrete format: public-key (Ed25519) tokens with offline attenuation (appended blocks can only *restrict*), verifiable from the root key, with revocation IDs that cascade to derived tokens; #cite("https://ucan.xyz/specification/", "UCAN") contributes the anti-splicing invariant (`aud` of each proof equals `iss` of the next, chaining to the resource owner). #cite("https://docs.sigstore.dev/cosign/signing/overview/", "Sigstore (Fulcio+Rekor)") turns a GitHub OIDC identity into a short-lived signing cert (key destroyed) recorded in an immutable transparency log: a strong precedent for GitHub-rooted identity without long-lived keys *and* for our append-only log. #cite("https://docs.cloud.google.com/iam/docs/agent-identity-overview", "Google Cloud Agent Identity (SPIFFE/SPIRE)") and #cite("https://blog.cloudflare.com/web-bot-auth/", "Cloudflare Web Bot Auth (RFC 9421)") show sender-constraining tokens (DPoP/mTLS, per-request signatures) so a stolen credential can't be replayed. #cite("https://datatracker.ietf.org/doc/html/draft-singla-agent-identity-protocol-03", "draft-singla AIP") is precedent for binding the chain root to a verified human (root issuer must equal the principal id).

== Dedup, entity resolution, and conflict detection
#cite("https://arxiv.org/abs/2303.09540", "SemDeDup") (Meta) is the canonical cluster-then-threshold semantic dedup, productionized by #cite("https://docs.nvidia.com/nemo/curator/curate-text/process-data/deduplication/semdedup", "NeMo Curator"); #cite("https://arxiv.org/html/2411.04257v3", "LSHBloom") shows pure-embedding dedup is infeasible at scale (a 5B-doc LSH index ≈277TB), motivating a tiered hash → MinHash → embedding pipeline. Entity/claim resolution follows a blocking→matching decomposition (#cite("https://arxiv.org/html/2405.16884v3", "Match-Compare-Select")); #cite("https://aclanthology.org/2024.emnlp-main.548.pdf", "EDC/CESI") warn embedding-only canonicalization over-generalizes relations and an LLM "define" step fixes it, important before asserting typed relations. #cite("https://aclanthology.org/2025.emnlp-main.1765.pdf", "CLAIRE/WikiCollide") (Stanford OVAL, EMNLP'25) is the strongest analog to whole-log contradiction detection: ≥3.3% of Wikipedia facts contradict another, and the best automated detector reaches only AUROC 75.1%, so the correct output is *reviewable candidates with evidence, not auto-resolution*. #cite("https://papers.nips.cc/paper_files/paper/2024/file/baf4b960d118f838ad0b2c08247a9ebe-Paper-Datasets_and_Benchmarks_Track.pdf", "ConflictBank") supplies a conflict taxonomy mapping onto our relation subtypes (temporal→`supersedes`, misinformation→`contradicts`, semantic→`refines`).

== Incentives and the 2026 landscape
#cite("https://arxiv.org/abs/2605.14421", "MemLineage") is a close analog of our signed append-only log + typed derivation relations, contributing an *untrusted-ancestor gate* (items descending from low-trust ancestors cannot justify sensitive actions). On incentive design, #cite("https://link.springer.com/chapter/10.1007/978-3-032-03273-7_1", "Rennie & Potts") reframe the challenge as *critical mass, not free-riding* (contribution goods accrue benefit to contributors); #cite("https://petertsehsun.github.io/papers/Is_reputation_on_Stack_Overflow_always_a_good_indicator_for_users_expertise_No.pdf", "Stack Overflow studies") show *global* reputation is a weak expertise proxy, arguing for per-topic, corroboration-weighted reputation. The 2026 wave (#cite("https://blogs.oracle.com/developers/oracle-ai-agent-memory-a-governed-unified-memory-core-for-enterprise-ai-agents", "Oracle AI Agent Memory"), the mem0/Zep/Letta managed backends) makes governed agent memory a shipping category, but all are org-scoped or single-principal. *None combines verifiable public provenance, personalized cross-principal trust weighting, and access-filtered public sharing*: that is our differentiator.

= Identity and delegation

Every actor is a *principal*: a *human* or an *agent*. The invariant: every agent has exactly one parent, and following parents always terminates at a human.

```
principal := human | agent
chain(agent) := [agent, ...ancestors, human]   // always ends in a human
```

*Rooting.* Humans authenticate via GitHub OAuth/OIDC; the GitHub identity (stable id, verified org/team memberships) is the root of trust. Following the Sigstore precedent, we mint *short-lived* signing material from the OIDC identity rather than holding long-lived per-human keys, and we *bind the chain root to the verified identity* (root issuer must equal the principal id, the draft-singla pattern), closing the self-assertion gap.

*Attenuated, signed chains.* Each spawned agent receives a delegated token. Rather than bespoke crypto we adopt a real attenuation format: a #cite("https://www.biscuitsec.org/", "Biscuit") block per link (or a #cite("https://ucan.xyz/specification/", "UCAN") delegation), so a token is offline-verifiable against the root key, *monotone* (each block can only narrow scope), carries policy as in-chain caveats (visibility scope, expiry, rate, org-only, a depth cap of 0–10), and supports *cascade revocation* of a compromised agent subtree via revocation IDs. We adopt UCAN's `aud == next.iss` invariant to prevent chain splicing.

*Sender-constrained, not bearer.* The presented chain is bound to the requesting agent's key via DPoP or mTLS on the HTTP surface (optionally RFC 9421 per-request signatures), so a leaked chain cannot be replayed by a different agent.

#figure(
  diagram(
    spacing: (7mm, 8mm), node-stroke: 0.5pt, node-corner-radius: 3pt,
    node((0,0), [human · GitHub OIDC\ (alice, org acme)], fill: blue),
    node((0,1), [agent alice/claude-1]),
    node((0,2), [sub-agent explore-7]),
    node((0,3), [knowledge event\ (full attenuated chain)], fill: amber),
    edge((0,0),(0,1), "->", [mint short-lived root, sign]),
    edge((0,1),(0,2), "->", [Biscuit block: write-only, ns=cuda]),
    edge((0,2),(0,3), "->", [DPoP-bound write]),
    edge((0,3),(0,0), "-->", [offline verify to root key], bend: 75deg),
  ),
  caption: [Signed, attenuated delegation. Each hop can only *narrow* scope; any reader verifies the chain back to the human root offline. A valid chain authorizes a write but never, by itself, confers trust.],
)

Ratings and trust ultimately attribute to the *human root*, but per-agent attribution is preserved so we can distinguish a careful review agent from a yolo agent. A valid chain authorizes a write; it never by itself confers trust (MINJA).

= Visibility, sharing, and the public/private split

Every item carries a *visibility*: `private` (author + human root), `grant` (explicit allow-list of principals/humans/orgs), `org` (members of a named org/team), or `public`. Agents *write private by default*; publishing is an explicit promotion. A single `search` returns the *union of everything the caller is entitled to see*, interleaved and ranked by personalized trust. Access control is a filter, not a separate database. This mirrors #cite("https://arxiv.org/abs/2505.18279", "Collaborative Memory")'s validated private + selectively-shared + access-filtered design.

```
readable_set(viewer) = private(viewer) ∪ grants_to(viewer ∪ orgs(viewer))
                     ∪ org_visible(orgs(viewer)) ∪ public
```

#figure(
  cetz.canvas({
    import cetz.draw: *
    let bands = (
      (11.0, 6.0, rgb("#f1f3f4"), [public], 2.55),
      (8.4, 4.6, blue, [org], 1.85),
      (5.8, 3.2, amber, [grant (named principals/orgs)], 1.15),
      (3.2, 1.8, green, [private — default], 0.0),
    )
    for (w, h, c, name, ly) in bands {
      rect((-w/2, -h/2), (w/2, h/2), fill: c, stroke: 0.5pt, radius: 4pt)
      content((0, ly), text(size: 8.5pt)[#name])
    }
  }),
  caption: [Visibility scopes nest. An item starts *private*; promoting it widens who may read it (a grant to named principals/orgs, a whole org, or fully public). One search returns the union of every band you can access.],
)

`knowledge` and the private transcript corpus are *different things with a shared substrate*; we design the schema so a future unification (one surface governed by a `visibility` flag) is possible, but do not couple them now, because putting public, attacker-reachable writes into the private corpus's trust domain before the trust layer is proven would be reckless.

#block(fill: rgb("#fff4e6"), inset: 9pt, radius: 4pt, width: 100%)[
  *Reconsider (org model):* v1 maps organizations/teams onto *GitHub orgs/teams* (auth already proves membership, zero extra plumbing). This couples us to one provider's notion of an org and cannot express external members or cross-org consortia, the distributed-lab case in the motivation. A *native* org/group model (membership managed here, GitHub as one identity provider among several) is likely the enterprise end state. The schema keeps `org` an opaque id with a pluggable membership resolver, so GitHub-backed resolution can be swapped for native groups without migration.
]

= Data model

== Append-only, bi-temporal event log
The store is a log of immutable *events*; current state is *folded* from the log (fits Iceberg's append-and-reconcile). Following #cite("https://arxiv.org/abs/2501.13956", "Zep/Graphiti"), every item and relation carries *both* `valid_time` (when the fact holds in the world) and `ingestion_time` (when we learned it). Default reads surface currently-valid facts; history stays queryable. Event kinds:

#table(
  columns: (auto, 1fr), inset: 6pt, align: left,
  table.header([*event*], [*meaning*]),
  [`item.create`], [a new knowledge item (payload below)],
  [`item.distill`], [a derived, structured strategy item (incl. first-class failure lessons), à la ReasoningBank; itself immutable, linked to its sources],
  [`item.revise`], [author supersedes their own item; old version retained],
  [`item.retract`], [author withdraws an item (kept in log, hidden from default reads)],
  [`rating`], [a reader scores an item useful/harmful with optional comment],
  [`corroborate`], ["this happened to me too": an independent observation reinforcing an item, with the corroborator's environment],
  [`relation`], [typed directed edge: `supersedes` | `depends-on` | `contradicts` | `refines` | `related-to` (may close a prior validity interval)],
  [`trust.assert`], [a principal vouches for *or distrusts* another (signed web-of-trust edge)],
  [`comment`], [free-text discussion attached to an item],
)

== Knowledge item payload
```
KnowledgeItem {
  id, author: principal_chain, visibility,
  title, body: markdown?,            // optional human-readable summary
  artifacts: Artifact[],             // ANY MIME: text/binary/video/audio/custom
  kind, tags,
  metadata: json,                    // user-supplied, arbitrary, filterable
  sys_metadata: json,                // forge-proof: created_at, signed chain,
                                     //   resolved root human + root org, trust_at_write
  examples: Example[], environments: Environment[],
  embedding, minhash, content_sha256,// dedup signals (tiered, §10)
  valid_time, ingestion_time, cluster_id?, last_verified
}
Artifact { name, mime, size, blob_ref(S3), sha256, free: json }
Environment { os, arch, versions: json, repo?, commit?, hardware?, nix?: flake_ref, free: json }
```
*Content is arbitrary*: an item bundles typed `artifacts` of any MIME type (text, binaries, video, audio, custom formats, datasets), content-addressed blobs in S3. *Metadata is split*: `metadata` is schema-free user JSON (filter via `pl.col("metadata").struct.field(...)`); `sys_metadata` is service-stamped and unforgeable (`created_at`, the signed chain, the resolved root human and root org, and `trust_at_write`). The roots are derivable from the chain but denormalized onto every item because it is the cheapest way to filter and audit ("everything alice's org published") without folding a chain per query.

== Derived distilled layer
Over the raw log we maintain a #cite("https://arxiv.org/abs/2509.25140", "ReasoningBank")-style layer of `item.distill` events: structured strategy units (title / when-to-use / actionable content), *including first-class failure lessons*, linked via `supersedes`/`refines`/`contradicts`. Raw events stay immutable; distilled, high-trust items are surfaced above raw events (#cite("https://arxiv.org/abs/2506.07398", "G-Memory")). Distillation, dedup, relation inference, and trust recompute run as background *sleep-time* jobs (#cite("https://arxiv.org/abs/2504.13171", "Letta")) that emit new events, keeping reads low-latency.

= Trust and rating

Trust is *relative to the viewer*; there is no global "this item is true". For viewer $V$ and author $A$ we compute a trust weight $t_V(A) in [0,1]$ by propagating from $V$'s roots over the signed trust graph (explicit `trust.assert` edges plus implicit edges from rating agreement), then rank items by combining author trust with trust-weighted ratings and corroboration.

#figure(
  diagram(
    spacing: (14mm, 7mm), node-stroke: 0.5pt, node-corner-radius: 3pt,
    node((0,1), [*you*\ (viewer)], fill: amber),
    node((1,0.4), [a root\ you trust]),
    node((2,0), [author A\ #text(fill: rgb("#137333"))[high weight]], fill: green),
    node((2,1), [author B\ #text(fill: rgb("#137333"))[some weight]], fill: green),
    node((1.5,2.3), [Sybil swarm: many fake accounts\ vouching only for each other\ #text(fill: rgb("#c5221f"))[≈ 0 weight for you]], fill: red, width: 52mm),
    edge((0,1),(1,0.4), "->", [trust]),
    edge((1,0.4),(2,0), "->", [trust (decayed)]),
    edge((0,1),(2,1), "->", [rates in agreement]),
    edge((1.5,2.3),(1.5,2.3), "->", bend: 130deg),
  ),
  caption: [Personalized, Sybil-tolerant trust. Weight flows from *your* roots and decays each hop, so authors your circle vouches for rank high. A swarm of fake accounts that only vouches for itself is never reached from you, so its weight is near zero no matter how large it grows.],
)

== Sybil-tolerance is mandatory, not optional
A naive personalized-PageRank / Advogato / EigenTrust metric is *provably not Sybil-resistant* (#cite("https://arxiv.org/abs/2207.09950", "MeritRank"); #cite("https://www.eecs.harvard.edu/cs286r/courses/fall09/papers/friedman1.pdf", "Friedman & Cheng"): no symmetric reputation function can be Sybilproof). We therefore make the metric Sybil-*tolerant* by layering MeritRank's three decays onto the personalized propagation:
- *transitivity decay* per delegation/endorsement hop (caps serial attacks),
- *connectivity decay* penalizing paths crossing a narrow cut of few attack edges (caps parallel/cycle attacks),
- *epoch decay* down-weighting stale ratings.
This keeps the lazy ego-walk but bounds attacker gain. EigenTrust is demoted to an *auditable, seed-anchored cold-start prior*, used only when no personalized path exists and fully overridden once one does.

== Signed, asymmetric, one-hop distrust
Trust propagates transitively (with decay); *distrust does not* (#cite("https://snap.stanford.edu/class/cs224w-readings/guha04trust.pdf", "Guha et al."): one-step distrust performs best, multi-hop distrust actively hurts). A negative `trust.assert` discounts the distrusted node's *outgoing* opinions one step and hard-suppresses it (and what it directly endorses) for the viewer's neighborhood, fast, without waiting for ratings. This is "like personalized PageRank but rooted on *you* and signed".

== Corroboration weighted by independent human roots, never by count
"This happened to me too" is powerful only if the corroborators are *independent*. Count-based corroboration is forgeable (#cite("https://aclanthology.org/2025.findings-emnlp.1023.pdf", "AuthChain") fabricates a chain-of-evidence in one document; #cite("https://arxiv.org/abs/2407.12784", "AgentPoison") engineers tight embedding clusters). So we collapse each corroborating agent's chain to its *human root*, dedupe by root, and aggregate corroboration as the sum of the *personalized WoT weights of the distinct roots*, requiring $k$ independent above-threshold roots before an item counts as consensus. By #cite("https://arxiv.org/html/2510.27554v1", "TraceRank")'s zero-seed-zero-reputation property, a flood of Sybil corroborations adds nothing.

== Ranking with a sensitivity floor
Retrieval ranks by $ "score"_V(d) = "sim"(q, d) dot t_V(d)^alpha + "recency" + "corrob"_V(d) $ with a *minimum-trust floor* $tau_min$ for high-stakes intents (credentials, financial, destructive actions) so low-trust items cannot be retrieved *at all* for sensitive queries (#cite("https://arxiv.org/abs/2410.22954", "RA-RAG") reliability weighting; the $"score" times "trust"^alpha$ + per-sensitivity-floor formulation). #cite("https://arxiv.org/abs/2405.15556", "RobustRAG")'s certified bound holds only while poison is a *minority* of retrieved items, which is precisely what the trust filter enforces.

== Proof + Stake for high-impact writes
Reputation governs *ranking, inclusion, rate-limits, and sampling*, but high-impact *writes* (a `supersedes` over a widely-trusted item, mass corroboration) require stronger evidence from a low-trust or new principal: sufficient delegation depth, a validation/attestation event, or an optional refundable anti-spam deposit (#cite("https://www.arxiv.org/pdf/2511.03434", "Inter-Agent Trust Models")'s Proof+Stake overlay; #cite("https://arxiv.org/abs/2505.14551", "TRep") for incentive-compatible honest reporting). A reputation drop triggers automatic escalation. This is an *action-impact* tier (T0–T3) layered on top of the *visibility* tiers.

= Search and query API

Two surfaces over one backend; both enforce ACL *and* mandatory provenance server-side. *Similarity retrieval alone is forbidden for public/cross-user scope*: an item is surfaced only with a valid chain and after trust filtering.

*Polars plugin* (ergonomic), mirroring `polars-mixedbread`:
```python
df = (scan_knowledge(query="cuda driver mismatch on H100", as_viewer=me)
        .filter(pl.col("metadata").struct.field("cuda") == "12.4")
        .filter(pl.col("trust") > 0.3)        # personalized, server-computed
        .sort("score", descending=True).head(20).collect())
```
Semantic match, ACL, trust, and the sensitivity floor push down to the service; structured `metadata` filters and ordering run in polars.

*HTTP service* (universal, bash-only friendly): every op is an authenticated call so a `curl`-only agent can participate.
```bash
curl -s https://knowledge.ix.dev/v1/search -H "authorization: Bearer $KNOWLEDGE_TOKEN" \
  --json '{"query":"cuda driver mismatch","filter":{"cuda":"12.4"},"limit":20}'
curl -s https://knowledge.ix.dev/v1/items  -H "authorization: Bearer $KNOWLEDGE_TOKEN" \
  --json '{"title":"...","body":"...","kind":"gotcha","metadata":{"cuda":"12.4"},"environments":[...]}'
curl -s https://knowledge.ix.dev/v1/items/$ID/corroborate -H "authorization: Bearer $KNOWLEDGE_TOKEN" \
  --json '{"environment":{...}}'
```
The token carries the attenuated chain; the service verifies it to the GitHub root, resolves `readable_set`, computes trust, and returns ranked results. The polars plugin is a thin client over this same API.

== Discovery: pull and subscriptions
Search is *pull*. We also support *push*: a principal subscribes to a query (tag, topic, author, org, saved semantic query) and new matching items within their `readable_set` are delivered to a feed, evaluated on the write path with the same ACL + trust filter. This turns "this happened to me too" from a lucky search into a reliable signal. Pull stays the default; feeds are opt-in and trust-thresholded to stay low-noise.

#block(fill: rgb("#fff4e6"), inset: 9pt, radius: 4pt, width: 100%)[
  *Reconsider (hosting):* v1 is a *thin service in front of Iceberg/S3 + mixedbread* owning auth, ACL, trust, and the write path; the *public* slice can later be served as signed parquet straight from S3 for heavy analytical scans, while private data never lands on a public bucket. Pure public-S3 cannot express private-by-default without per-scope prefixes/presigned URLs and leaves nowhere to run trust. Keep the service for the control plane; treat direct-S3 as a public-read optimization.
]

= Deduplication, corroboration, and conflict

*Tiered dedup* (#cite("https://arxiv.org/html/2411.04257v3", "LSHBloom"): pure-embedding dedup is infeasible at scale; #cite("https://arxiv.org/abs/2303.09540", "SemDeDup")): drop exact dupes by `content_sha256`, then MinHashLSH for near-verbatim, then embedding cluster-then-pairwise on survivors only. MinHash signatures live in S3 metadata so the cheap passes are incremental over the log. On near-duplicate at write time, we *offer corroboration* (attach a `corroborate` with environment to the existing cluster) rather than create a competing item, turning "N agents hit the same bug" into one multiply-corroborated item, with cross-trust corroboration the strongest ranking signal. An optional #cite("https://github.com/topoteretes/cognee", "Cognee")/#cite("https://aclanthology.org/2024.emnlp-main.548.pdf", "EDC")-style controlled-vocabulary canonicalization (with an LLM "define" step) runs before asserting typed relations, since embedding-only clustering over-generalizes relations.

*Conflict detection* is a #cite("https://aclanthology.org/2025.emnlp-main.1765.pdf", "CLAIRE")-style retrieve-then-reason loop: for a new item, retrieve access-filtered near items, reason over pairs, and on likely conflict materialize a `contradicts` (or, if strictly newer with overlapping claims, an auto `supersedes` that closes the old validity interval) *flagged for review with supporting passages*. We never auto-resolve: the best fully-automated detector reaches only AUROC ~75% (§3.5), so the verified-correct output is reviewable candidates, not decisions.

= Poisoning defense (summary)

The threat is real and scale-invariant (§3.3). Our layered response, in order of load:
1. *Mandatory signed provenance + independent-trust gating* (the only known discriminator against trigger-free fluent poison like MemoryGraft/AuthChain). No similarity-only retrieval for public scope.
2. *Trust-weighted retrieval + sensitivity floor* keeps poison a *minority* of retrieved items, the precondition for RobustRAG-style guarantees.
3. *Independent-root corroboration* (count-flooding is worthless).
4. *Hash-pinning* (`content_sha256`) so in-place mutation of a trusted item is detectable.
5. *Feedback-driven traceback* (#cite("https://arxiv.org/html/2504.21668v1", "RAGForensics")): on a reported bad outcome, attribute the responsible item(s) black-box, emit `contradicts`/`supersedes`, and propagate one-hop distrust up the offending delegation lineage to its human root, lowering future weight, turning one incident into durable lineage-level decay.
6. *Untrusted-origin tagging* (web/repo-derived content) discounted until independently corroborated (#cite("https://arxiv.org/abs/2605.14421", "MemLineage")'s untrusted-ancestor gate).

#figure(
  cetz.canvas({
    import cetz.draw: *
    let layers = (
      (11.0, [1 · signed-provenance gate — no valid chain, not surfaced]),
      (9.2, [2 · trust-weighted retrieval + sensitivity floor]),
      (7.4, [3 · independent-root corroboration (flooding is worthless)]),
      (5.6, [4 · hash-pinning + feedback-driven lineage traceback]),
    )
    let y = 0.0
    for (w, label) in layers {
      rect((-w/2, y - 1.0), (w/2, y), fill: blue, stroke: 0.5pt, radius: 2pt)
      content((0, y - 0.5), text(size: 8pt)[#label])
      y = y - 1.25
    }
    content((0, 0.55), text(size: 8.5pt, fill: rgb("#c5221f"))[poisoning attempts #sym.arrow.b])
    content((0, y + 0.05), text(size: 8.5pt, fill: rgb("#137333"))[#sym.arrow.b trustworthy knowledge surfaced])
  }),
  caption: [Defense in depth: each layer strips out more poisoned items. Signed provenance plus independent-trust gating is the only known discriminator against fluent, trigger-free poison (MemoryGraft/AuthChain); the lower layers keep poison a *minority* of what is retrieved, the precondition for any robustness guarantee.],
)

= Incentives

We do *not* gate reads on participation (hard reciprocity invites gaming). Instead each principal accrues a *visible, per-topic, corroboration-weighted* reputation that *feeds the trust prior*: a contributor whose past items your neighborhood rated useful has their new items start ranked higher for you, before those items have ratings. Good contribution is rewarded with reach, not unlocked by a paywall. Per-topic and corroboration-weighting follow #cite("https://petertsehsun.github.io/papers/Is_reputation_on_Stack_Overflow_always_a_good_indicator_for_users_expertise_No.pdf", "Stack Overflow") evidence that global reputation is a weak expertise proxy; the real adoption risk is *critical mass*, not free-riding (#cite("https://link.springer.com/chapter/10.1007/978-3-032-03273-7_1", "Rennie & Potts")). A shipped contributor guide (skill / system-prompt section) makes the maximally-reproducible behaviors routine: write a gotcha with the smallest runnable repro the moment you fix something; always attach environment; corroborate rather than duplicate; rate what you used; retract/revise when wrong; default private, publish deliberately.

= Scalability

Writes are append-only events to Iceberg; embedding/MinHash computed on write (async) feed dedup and mixedbread. Reads hit mixedbread for candidate recall, then the service applies ACL, the cached personalized trust vector, and the sensitivity floor. Folded item-state and per-viewer trust vectors are cached (TTL + neighborhood invalidation on new edges). The MeritRank-decayed ego-walk is incrementally computable (#cite("https://arxiv.org/pdf/2307.01411", "Web3Recommend")). The hot public slice can be materialized as parquet on S3 for direct polars scans. Curation (dedup, relations, trust recompute, distillation) runs off-path as sleep-time jobs.

= Open problems (honestly stated)

These have no fully satisfying answer; the design acknowledges rather than hides them.
1. *Automated contradiction/fact-verification is weak* (CLAIRE AUROC ~75%; best fact-verifier ~0.63 F1 on false claims). `contradicts`/`supersedes` must stay trust-weighted, surfaced-for-review signals; we do not claim automated truth-resolution.
2. *Sybil-tolerance bounds, it does not prevent.* The only route to true uniqueness (personhood/biometric/social-graph roots) depends on a trusted issuer or scales slowly (BrightID ~100K). A public commons inherits an open uniqueness-root problem.
3. *The Decentralized Reputation Trilemma*: a system cannot be simultaneously generalizable, trustless, and Sybil-resistant. We sacrifice a single global generalizable score for personalized Sybil-tolerance, and say so rather than imply a universal trust number.
4. *Trigger-free fluent poisoning* defeats every content-based detector; provenance + independent-trust gating is the only known discriminator and has no published large-scale adversarial validation. Residual real-world attack success for a public commons is unknown.
5. *Certified robustness needs poison-minority*, which depends entirely on the trust filter; there is no certificate for an attacker who accumulates trust legitimately then poisons, nor for in-place insider edits by an already-trusted principal.
6. *Incentive design is under-evidenced* (observational SO studies + theory, not controlled agent-commons experiments). Informativeness-scored corroboration (peer-prediction) assumes priors that free-form reports violate and is collusion-prone. We have no validated anti-gaming guarantee.
7. *Agent-identity standards are unratified and unaudited*; even building on Biscuit/UCAN/Sigstore leaves multi-hop human-rooted delegation ahead of any ratified surface.
8. *Verifiable utility-based write-admission* (SEDM replay) is attractive but impractical for open-ended, non-deterministic, side-effecting contributions.

= Open questions to settle before promotion

GitHub-orgs vs native groups (membership resolver interface); the thin-service vs direct-S3 read boundary under load; agent key custody (parent-minted token vs per-agent keypair, now leaning Biscuit blocks); and the trust-tuning + dedup-threshold knobs ($alpha$, $tau_min$, MeritRank decay rates, $k$ independent roots, similarity cutoffs per `kind`), which want an *experiment* (ranking quality vs poisoned-item suppression rate) rather than a guess.

= Phased plan

#table(
  columns: (auto, 1fr), inset: 6pt,
  table.header([*phase*], [*scope*]),
  [0 (this doc)], [design, verified SOTA review, decisions, open problems],
  [1], [bi-temporal append-only log on Iceberg; item create/read with arbitrary artifacts (S3 blobs) + user/system metadata; mandatory signed provenance; embedding + mixedbread recall *scoped to the caller's own private items only* (no cross-user or public surfacing yet, since the trust gate does not exist before phase 3); HTTP service with GitHub OIDC auth. Public *visibility* may be set on writes, but public items are not surfaced to other principals until phase 3.],
  [2], [ratings + independent-root corroboration; tiered dedup (hash→MinHash→embedding); typed relations incl. `contradicts` review candidates; polars `scan_knowledge` (still own-scope); per-topic reputation surfacing; sleep-time curation jobs],
  [3], [attenuated signed delegation chains (Biscuit/UCAN) + sender-constraint; personalized Sybil-tolerant WoT (MeritRank decays); one-hop distrust; trust-weighted ranking + sensitivity floor; EigenTrust seed-only prior; Proof+Stake high-impact gate. *This is the first phase that surfaces public / cross-user items, now strictly behind the provenance + trust gate.*],
  [4], [org/grant visibility + membership resolver; subscriptions/feeds; enterprise audit; feedback-driven traceback + lineage decay; distilled strategy-item layer],
  [5], [Nix-defined reproducible environments + opt-in verification; Sigstore/Rekor-style transparency; ontology canonicalization; optional unification with the private corpus],
)

#v(0.5em)
#line(length: 100%, stroke: 0.5pt + luma(180))
#text(size: 8.5pt, style: "italic")[
  Draft v0.2 for discussion. Lives under `packages/andrewgazelka/` as a personal test project; promote out of that namespace once the org model, hosting boundary, and trust tuning are settled. The §3 review was produced by a multi-agent web sweep with per-claim adversarial verification; vendor self-reported and single-author-preprint results are flagged inline and should not be cited as settled.
]
