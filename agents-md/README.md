# AGENTS.md fragments

This directory owns the reusable Markdown fragments behind the generated
[`AGENTS.md`](../AGENTS.md). The default renderer keeps this repository's
checked-in file byte-for-byte reproducible:

```sh
nix run .#agents-md
nix run .#agents-md -- --write AGENTS.md
nix run .#agents-md -- --check AGENTS.md
```

Other repositories can consume `lib.agentsMd` and choose the sections they
want:

```nix
index.lib.agentsMd.render {
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
}
```

Keep broad guidance in a named fragment. Put one-off repository facts in
`extraSections` or in that repository's own fragment list.
