---
name: layout
disclosure: progressive
description: "Repository directory layout and where each kind of file lives. Use when unsure where a file belongs or how the tree is organized."
---

## Layout

```
flake.nix                                  # manifest: inputs + delegated outputs
.envrc, .githooks/pre-commit               # direnv wires the tracked hook
lib/                                       # public helpers, builders, discovery
modules/                                   # registered NixOS modules and profiles
images/                                    # image modules plus optional versions
nix-rules/                                 # ast-grep lint rules
```

Folders should preserve conceptual paths. When siblings share a real domain,
nest them under that domain instead of flattening the name into repeated dashed
prefixes. Published package names, image tags, and upstream identifiers can keep
their external spelling.

Move a legacy flat path while doing nearby work when the rename is small and
call sites are inside the repo. Leave a follow-up when the rename is larger than
the work that exposed it.

