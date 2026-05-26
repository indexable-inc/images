# AGENTS.md fragments

This directory owns the reusable Markdown fragments behind the generated
[`AGENTS.md`](../AGENTS.md) and [`CLAUDE.md`](../CLAUDE.md). The default
renderer keeps this repository's checked-in instruction files byte-for-byte
reproducible:

```sh
nix run .#agents-md              # diff generated files against the checkout
nix run .#agents-md -- --write   # write AGENTS.md and CLAUDE.md
nix run .#agents-md -- --check   # fail if either file is stale
nix run .#agents-md -- --diff-renderer plain
nix run .#agents-md -- --target codex --print
```

The no-argument diff uses `delta` when stdout is an interactive terminal and
falls back to plain unified diff text for pipes and logs.

Other repositories can consume `lib.agentsMd` and choose the sections they
want. Pass `target`, `targetSections`, or `extraSectionsByTarget` when Codex
and Claude need different rules:

```nix
index.lib.agentsMd.render {
  target = "codex";
  enabledSections =
    index.lib.agentsMd.profiles.common
    ++ index.lib.agentsMd.profiles.nix
    ++ [
      "layout"
    ];
  extraSections = [
    ''
      ## Local notes

      Put repository-specific guidance here.
    ''
  ];
  extraSectionsByTarget.claude = [
    ''
      ## Claude-only notes

      Put Claude-specific guidance here.
    ''
  ];
}
```

Keep broad guidance in a named fragment. Put one-off repository facts in
`extraSections`, `extraSectionsByTarget`, `targetSections`, or in that
repository's own fragment list.
