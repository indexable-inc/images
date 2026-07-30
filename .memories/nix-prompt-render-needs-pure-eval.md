---
tldr: A broken Rust workspace makes the flake prompt render fail; the pure-eval path renders it without cargo
genre: memory
topic: [nix, tooling]
handle: [claude-code.systemPrompt, cargo-unit-graph, agent-prompt]
prior: 0.5
based_on:
  - path: packages/agent-prompt/default.nix
    blake3: 34d9ac2d501a58ad
validated:
  - at: 2026-07-30T03:04:25Z
    by: claude-opus-4-6
    how: nix eval --raw .#claude-code.systemPrompt failed on cargo-unit-graph.json.drv; the --impure --expr path rendered 23703 bytes, rc=0
    ok: true
---
`nix eval --raw .#claude-code.systemPrompt` renders the copy baked into the
wrapper, and it depends on the whole Rust workspace through
`cargo-unit-graph.json.drv`. So an in-progress crate that does not build makes
the prompt render fail with an error naming cargo, not the prompt, and it reads
as "my rule broke the render".

Render without touching cargo, which is what `packages/agent-prompt/default.nix`
documents at the top of the file:

    nix eval --raw --impure --expr \
      '(import ./packages/agent-prompt { lib = (import <nixpkgs> {}).lib; }).systemPrompt'

Use the pure path while iterating on a rule, and the flake path once the
workspace builds, because only the flake path proves what the wrapper ships.
Both are needed: the pure one cannot see `omitRules`.
