---
name: synthesize
description: Iterative research-and-design loop that converges on the best answer to an open architecture or technical-decision question. Use when the user wants to think through a hard design choice, explore approaches, weigh tradeoffs, "find the best way to do X", produce an RFC/ADR/proposal, or asks to research a topic and iterate to a recommendation. Grounds in Exa web search plus the codebase, drafts a proposal, has a separate read-only critic (its own isolated context, independent research) tear it apart against a frozen rubric, refines, and loops a bounded number of times until it converges, then emits an ADR-shaped recommendation. Fires on phrasings like "what's the best architecture for", "research X and propose", "think through", "iterate until you find the best", or "synthesize".
---

# Synthesize: research → propose → critique → refine → converge

For an open question with real tradeoffs, one pass underuses the model: it commits to the first plausible answer and never plays devil's advocate against itself. This skill is the structured loop that fixes that. Ground the question in evidence, draft a proposal, have a **separate** critic break it against a **frozen** rubric, refine only what the critic flagged, and stop the moment the loop runs out of signal. The output is a decision memo (ADR-shaped), not a chat recap.

This is grounded in the published shape of self-refine / reflexion / generator-critic loops (Madaan 2023, Shinn 2023, Anthropic's effective-agents writing, the agent-patterns catalog). Two findings drive every rule below: **the gain curve is concave and short** (iteration 0→1 is the big lift, 2 is smaller, 3 is the cap, 4+ regresses through over-editing), and **same-model self-critique rubber-stamps** (a model that just wrote and defended text treats it as committed work and approves it). So we bound iterations hard and separate the critic from the author.

## When to use it (and when not)

Use it when the decision is **reversible-but-expensive, multi-dimensional, and groundable**: there's real code or literature to anchor on, and being wrong costs days. Examples: "should this be one binary or three crates", "what's the right cache invalidation strategy here", "pick a wire format for the SDK boundary".

Skip it when the answer is obvious (best practice clearly applies, no debate needed), when it's a pure fact lookup (just search), or when evaluation needs knowledge the model lacks (novel domain facts, your private business logic). On ungroundable questions the loop polishes a confident wrong answer into a more confident wrong answer. If you can't ground the critique in something checkable (code, a benchmark, a cited source), say so and don't pretend the loop adds rigor.

## The procedure

### 0. Frame and freeze the rubric (before any looping)

State the question in one sentence and write down the **success criteria as a fixed checklist** of 3 to 7 items. This rubric is frozen for the whole run. The critic scores against *these* and may not invent new criteria each round (free-form criteria is what makes critique drift and rubber-stamp). Criteria must be near-binary and checkable, e.g. "handles the 10x-scale case without a rewrite", "no new operational surface the team can't run", "every claim cites code or a source". Avoid "rate elegance 1-10": scores aren't actionable.

### 1. Ground (this is what makes the loop honest)

Gather evidence *before* proposing. In parallel:
- **External**: `mcp__exa__web_search_exa` for prior art, best practices, and known failure modes. Describe the ideal page, not keywords. Follow up with `web_fetch_exa` on the best 1-3 URLs when highlights are thin. Scope to ~5 results, this is triage not a dump.
- **Internal**: search the codebase (`mgrep search -a --agentic` for semantics, `rg` for exact strings) for how the relevant thing is done today, existing constraints, and prior decisions. Recall any relevant memory.

Every claim in the proposal must later trace to one of these. Mark internal evidence `[code: path:line]` and external `[src: short-name]`.

### 2. Propose (draft v0)

Write the first proposal: the recommended approach, **2-3 genuine alternatives** (not strawmen), and for each the tradeoffs. Be concrete and cite grounding. This draft sets the ceiling, so spend real effort here, but do not polish, the critic is next.

### 3. Critique with a SEPARATE, read-only critic (the load-bearing step)

Never critique your own draft inline, and never let the critic edit anything. Spawn a fresh critic via the Agent tool in its **own isolated context**, so it does not inherit your reasoning or your search trace. Its blind spots are then decorrelated from yours and it can't rationalize a defect it would have committed.

Use the dedicated **`synthesis-critic`** agent (`subagent_type: synthesis-critic`). It is purpose-built for this step: structurally read-only (no Edit/Write tools, so it *can't* write rather than being asked not to), it grounds independently (its own Exa search and code reads), scores against the frozen rubric only, and ends with a `PASS`/`REVISE` token. Hand it **only the proposal text and the frozen rubric**, nothing about how you got there. (If `synthesis-critic` is unavailable, fall back to `code-reviewer` for code-heavy decisions or `Explore` otherwise, both read-only; avoid `general-purpose` since it *can* write.)

The critic returns severity-ranked findings (quoted span, problem with its own evidence, concrete fix, scored 1 to 5) and the verdict token. The critic researches and finds; **you, the author, apply the fixes** in step 4. That split is the whole point: it stops the roles from collapsing back into one voice that rubber-stamps itself.

For high-stakes or genuinely two-sided calls, run it as a **debate**: one read-only critic argues the proposal is wrong (find the fragility), then a builder pass defends or adapts, both seeing the full exchange. Tension beats premature consensus.

### 4. Refine (surgically)

Address only findings at **severity >= 3**. Touch only the flagged spans. Do **not** "rewrite the whole thing in light of the review", that is exactly where over-correction comes from: the revision step wrecks parts that were already fine. Minor/trivial notes are logged, not acted on. If a fix needs more grounding, loop back to step 1 for that one point.

### 5. Halt (stop early, on purpose)

After each refine, stop if **any** of:
- the critic returned `PASS` (no finding >= severity 3), or
- **no progress**: the new draft is materially the same as the prior one, or
- **oscillation**: the draft is reverting toward an earlier version, or
- you hit **iteration 3** (hard cap).

Default expectation: **converge in 1 to 2 critique rounds.** The 0→1 pass is the real lift; round 2 catches the rest; round 3 is the ceiling and usually a no-op. Do not run a third round "to be safe", that's hope, not signal. If you ever feel pulled to a 4th round, the loop has nothing left to find: stop and report what's genuinely unresolved instead.

### 6. Emit the decision memo (ADR-shaped)

Output, concisely:
- **Decision**: the recommended approach, one or two sentences.
- **Context**: the question and the binding constraints (cited).
- **Alternatives considered**: each rejected option and the *specific* reason it lost.
- **Tradeoffs / consequences**: what this buys and what it costs, including the risk that ages worst.
- **Open questions**: what stayed unresolved and what evidence would settle it. Don't manufacture false confidence.

Keep it tight. Lead with the decision. This memo is the deliverable; offer to drop it into an RFC/ADR file or a Linear issue if the user wants it persisted.

## Failure modes to watch (from the literature)

- **Rubber-stamp critic**: if critiques sound vaguely positive and each revision is cosmetic, the critic isn't separated enough or the rubric is too loose. Tighten to "list at most 5 problems, severity 4-5 first, quote the exact span."
- **Over-correction**: if the refined draft is worse (a judge would prefer the prior), the revise step drifted into rewrite-everything. Re-scope it to the flagged spans only.
- **Runaway loop**: never "iterate until the critic is happy", a stubborn critic rephrases the same complaint forever. The iteration cap and no-progress check are the safety belt, not a smarter critic.
- **Polishing a wrong answer**: on ungroundable points the loop adds confidence, not correctness. If the critique can't cite anything checkable, flag the point as unresolved rather than refining it.
