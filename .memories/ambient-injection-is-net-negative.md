---
tldr: On this fleet, deliberate prior-search paid 4-8x while injecting context into ordinary prompts measured net-negative
genre: memory
topic: [architecture, agency]
handle: [prompt-priors, context-digest, distilled_facts]
prior: 0.5
validated:
  - at: 2026-07-30T03:04:25Z
    by: claude-opus-4-6
    how: read docs/_archive/design/context-research.html sections 7 and 10; numbers quoted verbatim
    ok: true
---
The only measurement this fleet has on agent memory, from a 2026-06-12 study
(14 agents, live store queries, headless A/B):

- **Deliberate search pays.** 10 recurring task types, 8 of 10 returned useful
  priors: ~53k injected tokens against 220-400k tokens of avoided rediscovery,
  so 4-8x. In the A/B the priors arm answered 2 of 2 where cold answered 1 of 2,
  at 62% fewer input tokens and 56% fewer tool calls. Caveat from its authors:
  n=2, one run per arm, directional not powered.
- **Ambient injection does not.** 3 of 5 casual prompts pulled 0.3-9k tokens of
  pure noise, scoring 0.64-0.67 against winners at 0.71+. It broke even only
  score-gated at 0.70, source-prioritised, capped near 1200 tokens and silent on
  a miss. Its own conclusion: "Session-start digests must come from distilled
  facts, never live vector hits."
- **The ceiling is ingestion, not retrieval.** Indexing every message meant
  multi-hundred-KB tool-result logs dominated any failure-flavoured query, and
  one file from three checkouts appeared as three to five duplicate hits. Better
  ranking does not fix a corpus of raw logs.

This is why `.memories` has no `always:` field and nothing injects at session
start: an agent searches because the prompt tells it to. It is also why the
format caps body size and lints duplicate `tldr`s, which are the two levers that
study actually measured.

Source: `docs/_archive/design/context-research.html` in the ix repo, which is in
an archive directory. ENG-11402 asks for these numbers to be moved somewhere they
will survive.
