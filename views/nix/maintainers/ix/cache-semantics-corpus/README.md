# Four expressions the lang corpus does not have

`cache-semantics-gate.sh` differs along a *setting* rather than along an
evaluator, so each of its configurations is only worth running if some
expression answers differently under it. Measured against
`tests/functional/lang` alone, two of them were not: no corpus file calls
`builtins.nixVersion`, and none recurses deep enough for a call-depth ceiling
of 100 to bite. Those arms passed while witnessing nothing, which the gate's
arm 2 reported and this directory fixes.

One file per setting, named for it:

| file | witnesses |
|---|---|
| `eval-okay-cachesem-nix-version.nix` | `ixe_set_nix_version` |
| `eval-okay-cachesem-store-path.nix` | `ixe_set_store_dir`, through an `outPath` |
| `eval-okay-cachesem-current-system.nix` | `ixe_set_current_system` |
| `eval-okay-cachesem-deep-recursion.nix` | `ixe_set_max_call_depth` |
| `eval-okay-cachesem-read-a-file.nix` | `ixe_set_pure_eval`, `ixe_set_restrict_eval` |
| `eval-okay-cachesem-warns.nix` | that a warning survives memoisation |

The `eval-okay-` prefix is what the harness discovers on, matching
`lang-diff.sh`; the `cachesem-` infix keeps these from colliding with a lang
corpus file of the same name, since outcomes are keyed on the basename.

These are deliberately not added to `tests/functional/lang`. That corpus is
compared against cppnix and every file in it needs an `.exp`; these exist to
make one setting move one answer, and three of the four answer differently on
every machine.
