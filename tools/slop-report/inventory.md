# Slop inventory (Phase A)

Phase A output for the aggressive deletion pass. Every row is "looked at" —
keepers are noted, deletables are tagged. Phase B (human review) decides what
proceeds to Phase C.

## Method

- Walked owners: `lib/`, `modules/`, `packages/`, `site/`, `tests/`,
  `tools/`. Out of scope: generated manifests under `lib/data/`, the
  `codex/discover-tree` work itself, `nixosModules.*` / `packages.*` /
  `ix.lib` public option names (frozen surfaces).
- Each candidate is "verified" (I personally read the call sites) or
  "agent-flagged" (a subagent flagged it but I have not yet re-checked).
  Anything in this file is a *proposal*, not a deletion.
- `bytes_removable` is a rough estimate. Lines saved are usually a better
  ground truth.

Categories (from the slop-hunt brief):
1. Defensive validation the type system / module merge already does
2. Wrappers with one caller and no added value
3. Premature generality (single-value parameters, dead pluggable interfaces)
4. Re-implementations of `lib.*` / `builtins.*`
5. Stringly-typed plumbing
6. Comments restating code; stale TODOs
7. Mid-layer indirection that doesn't transform
8. Dead branches (`mkMerge [X]`, `optional true`, `mkIf` over already-gated)
9. Custom derivations where a builtin would do
10. Verbose error helpers that obscure the source
11. Test slop (mirrors source; eval-only no-assert)
12. Dead public items (`pub` with no out-of-crate caller)
13. Comments / docstrings that restate code

A separate column flags whether a deletion would change a store path
(generally no, unless it touches a build phase or fileset filter).

---

## Owners

- [`lib/`](#lib)
- [`modules/`](#modules)
- [`packages/`](#packages)
- [`site/`](#site)
- [`tests/`](#tests)
- [`tools/`](#tools)

---

## lib/

Verified candidates (re-read after the lib subagent flagged them):

| file:lines | cat | bytes | confidence | note |
| --- | --- | --- | --- | --- |
| lib/per-system.nix:333-367 | 7 | ~220 | verified | Three `lib.optionalAttrs (system == ix.system)` blocks merge into one wrapper around the three attr groups. Mechanical refactor, no store-path change. |
| lib/rust-tooling.nix:20-28 | 2,7 | ~180 | verified | `ixBuildSurfaceFor` and `llmClippyFor` each have one caller in the same `let`. Inline both. Composition stays expressive; the names are not load-bearing. |
| lib/relative-path.nix:15 | 7 | ~80 | verified | `renderPath` has one caller (line 19). Inline as `if builtins.isString path then path else "<${builtins.typeOf path}>"`. |
| lib/health-checks.nix:44-66 | 7 | ~250 | verified | `ixTokenCheck` and `ixTokenPrompt` are each interpolated once. Inline at the call sites in `mkLifecycle` and `zellij`. |
| lib/build-svelte-site.nix:51-59 | 10 | ~280 | verified | `checkedPackageManager` re-spells `errors.assertEnum`. Wire `errors` into the file via lib/default.nix and replace. Saves an inline `throw` and a 4-element let. |
| lib/build-npm-vitest.nix:96-115 | 4 | ~250 | verified | `regexSpecialChars` + `escapeRegex` re-implement `lib.escapeRegex` (nixpkgs lib has this). `exactNamePattern` stays; the local helper goes. |
| lib/discovery.nix:14-24 | — | 0 | **kept** | `mergeNestedAttrs` looks like a `lib.zipAttrsWith` re-spell, but the duplicate-name `throw` is real and `lib.zipAttrsWith` does not provide it. Leave with a one-line comment if the next reader is tempted. |
| lib/secrets.nix:38-66 | 1 | 0 | **kept** | Four boundary asserts on `provider.type`, `mountRoot`, `key`, `path`. Validates user fleet input from `mkFleet`; not redundant with the type system. |
| lib/uv-lock.nix:24-29 | 1 | 0 | **kept** | Asserts on parsed `uv.lock` shape, which is foreign data. Keep. |
| lib/agents-md.nix:1-272 | — | 0 | **kept** | Public surface; the long `/** */` doc-comments are mandated by AGENTS.md for `ix.lib` bindings. Do not delete. |

Subagent-flagged but unverified (Phase B should decide whether to verify
before deletion):

| file:lines | cat | bytes | confidence | note |
| --- | --- | --- | --- | --- |
| lib/minecraft-sync-managed.nix:19-50 | 7 | ~800 | unverified | `rootArgs`/`reloadArgs`/`ignoredPluginArgs`/`datapackWorldArgs`/`rconArgs` claimed to be one-shot intermediate lists. Re-read needed; if true, inline into the final args list. |
| lib/build-gradle-fat-jar.nix:44-127 | 4 | ~1600 | unverified | Hand-rolled POM parser; agent suggests `nixpkgs` provides XML parsing. Real XML parsing is nontrivial; risk of behavior change is real. Triage carefully. |
| lib/health-checks.nix:67-129 | 7 | ~320 | unverified | `mkLifecycle` is conceptually a per-fleet map (`lib.mapAttrs mkLifecycle exampleFleets`), so the "one caller" framing is misleading; likely **kept**. |
| lib/artifacts.nix:33-47 | 7 | ~180 | unverified | `generatedCatalogs` has three internal callers (mods, paper, velocity). Real dedupe; likely **kept**. |
| lib/build-zig-package.nix:87-94 | 7 | ~250 | unverified | `zigCacheScript` shell fragment; if used twice (install + test phases) keep, if once inline. |

Subagent claims that look wrong on a spot-check (rejected without entering
Phase B):

- lib/artifacts.nix:61-112 — `readLoaderManifest` schema validation flagged
  as "defensive". It's structural validation against foreign JSON; that's
  exactly when assertions earn their keep.
- lib/rust.nix:128-172 / 255 — same pattern, `checkedGitOutputHashes`
  validates vendored sources; non-redundant.

---

## modules/

The modules subagent was over-aggressive: many of its "category 1" hits
re-spelled real cross-option semantic checks the type system cannot
express, and many of its "category 2" hits proposed inlining module
options that are part of the user-facing public surface for operators.

Verified candidates:

| file:lines | cat | bytes | confidence | note |
| --- | --- | --- | --- | --- |
| modules/services/git-clone/default.nix:57-96 | 8 | ~120 | verified | `systemd = mkMerge [ {services.git-clone = ...} (mkIf timer {timers.git-clone = ...}) ]` can flatten to `systemd.services.git-clone = ...;` plus `systemd.timers = mkIf ... { git-clone = ...; };`. Saves a level of indent and the `mkMerge` wrap. |
| modules/services/git-clone/default.nix:3 | 6 | ~70 | verified | `# TODO: use cross-VM shared CAS to significantly speed up clones` — orphaned (no owner, no date). Convert to a GitHub issue with `enhancement` label and delete the comment. |
| modules/services/minecraft/default.nix:89 | 7 | ~80 | verified | `unsafePathShapes` has one caller (line 663). Inline as `lib.filter (path: !isSafeRelativePathShape path) annotatedWorldNames`. Siblings `unsafePaths` (3 callers) and `unsafeNames` (2 callers) stay. |
| modules/services/minecraft/default.nix:33-56 | 4 | ~300 | verified | `flattenProperties` mostly re-spells `lib.attrsToList`; the duplicate-name guard is the only piece that adds value. Worth inspecting whether `lib.concatMapAttrs` + a single dedupe would shorten it. Triage carefully — used in property rendering. |
| modules/services/minecraft/default.nix:1146-1148 | 8 | ~120 | verified | `mkIf (builtins.isString (cfg.properties.motd or null))` is defensive against a freeform type. The `freeformType = formatValueType` on `properties` allows any JSON value, so the guard is real but ugly. Borderline; leave with a comment naming the freeform shape. |

Subagent-flagged but rejected on spot-check (kept):

- modules/services/minecraft/default.nix:1120-1141 (5 assertions): all
  semantic cross-option checks (duplicate UUIDs, mutual-exclusion of
  src/generated datapacks, worldBorder requires RCON, etc.). Each
  catches a state the type system cannot.
- modules/services/remote-desktop/default.nix:143-156 (2 assertions):
  real cross-option sync for `bind-tcp` and `openFirewall` + `auth`.
- modules/services/observability/default.nix:389-398 (2 assertions):
  real semantic guards (at-least-one-exporter, agent-only needs endpoint).
- modules/services/geyser/default.nix:80-110 (38 `"snake-case" = cfg.camelCase`
  mappings): these are the YAML emitter contract. Sugaring them risks
  obscuring the mapping for the next reader. Leave inline.
- modules/services/{minecraft,velocity,observability,floodgate}/default.nix
  large option blocks: these *are* the public operator surface. Not slop.

---

## packages/

The packages subagent flagged four "dead" functions in
`packages/ix-dev-diagnose/src/main.rs` that are in fact called four times
each (verified). Treat the rest of its output with the same skepticism.

Verified candidates:

| file:lines | cat | bytes | confidence | note |
| --- | --- | --- | --- | --- |
| packages/file-search/src/types.rs:43 | 12 | ~30 | verified | `#[derive(Clone)]` on `SearchResult`. No `.clone()` call exists in this crate; the type is exported, but no external consumer. Drop `Clone`. |
| packages/file-search/src/ephemeral.rs:17 | 12 | ~30 | verified | `#[derive(Clone)]` on `RankResult`. The struct also derives `Copy` (it's `(usize, f32)` in shape), making `Clone` redundant. |
| packages/registry.nix:146-147 | 12 | ~50 | verified | `defaultPackageDirs` and `packageDirs` are exported but only used internally (only `packageDirsWithoutMetadata` has an external consumer in `tests/default.nix`). Drop from the export. |

Spot-checked false positives from the agent:

- `packages/ix-dev-diagnose/src/main.rs:760,1046,1058,1083` —
  `parse_status_line`, `unknown_issuer`, `unexpected_http_status`,
  `probe_ok` are all called (lines 746, 1009, 1012, 1021 respectively).
  Reject.

Not investigated (Phase A coverage gap; Phase B should approve a deeper
pass before Phase C touches these):

- packages/nix-cargo-unit/src/render.rs (4494 lines) — too large for a
  reliable subagent pass; needs a targeted human read of the public-API
  surface (`pub` items, `mod` boundaries) and the `cfg(test)` block.
- packages/dag-runner/src/main.rs (978 lines)
- packages/oci-image-builder/src/main.rs (941 lines)
- packages/run/run.py (912 lines)
- packages/ix-fleet/src/ix_fleet/__init__.py (847 lines)

---

## site/

| file:lines | cat | bytes | confidence | note |
| --- | --- | --- | --- | --- |
| site/src/lib/diagrams/RoomServerDiagram.svelte (whole file, 56 lines) | 12 | 2075 | verified | Component is not imported anywhere in `site/src`. Delete the file. |
| site/src/lib/UpdateEntry.svelte:10-13, 19-20, 36-45 | 3,8 | ~200 | verified | `titleLinksToPermalink` is only ever the default `true`; the third branch (h2 without link) is unreachable. Drop the prop and the branch. |
| site/src/lib/diagrams/DiagramFrame.svelte:54 | 5 | ~110 | unverified | Broad TS cast `as unknown as Record<string, typeof BoxNode>`. Narrow at the call site. |
| site/src/lib/filter-expression.ts:172-214 | 3 | ~1280 | unverified | `highlightExpression()` + `HighlightToken` produce a `tok-space` class that has no matching CSS. If the syntax-highlight overlay is unused, drop it; if used, ship the missing rule. |

---

## tests/

The tests subagent badly misread the "eval-only no-assert" category:
many of its category-1 hits are `tryEval` *setup* bindings whose result
is asserted on (`!success`) downstream. Those are real tests of the
assertion path. Treat the subagent's output as a list of files to
re-examine, not as proposals.

Verified rejects:

- `tests/default.nix:753-841` and surrounding `goUnit*Eval` bindings:
  these are paired with `assertion = !X.success` checks at
  ~2880-2906. They protect the assertion path — a regression where
  someone removes a `lib.assertMsg` would flip the result silently.
  Keep.
- `tests/default.nix:3627-3645` `cargoUnitRealWorkspaceAssertions`:
  proves cargo-unit handles serde's proc-macro lib, thiserror's derive
  impl, indexmap's workspace tests, regex's CLI binary, and regex-syntax
  tests. Each is a real cross-boundary contract. Keep.

No verified deletion candidates from tests this pass. The right next
move for tests is a focused human read of `tests/default.nix` looking
specifically for category-11 issues (mirrors source); a subagent run
was not productive here.

---

## tools/

| file:lines | cat | bytes | confidence | note |
| --- | --- | --- | --- | --- |
| tools/update-loaders.py & tools/update-mods.py (multiple sites) | 2,7 | ~1500 | unverified | Tools subagent flagged ~17 small wrappers and intermediate-variable chains (`remember_selected_version`, `latest_papermc_build` flattening, `summarize_file`). Many overlap conceptually; some may collapse to a shared helper across both files (`refresh_papermc` + `refresh_fabric` share structure). Triage before deletion — these scripts are user-facing and behavior must not drift. |

Verified rejects:

- `tools/update-loaders.py:24`, `tools/update-mods.py:14` — agent
  claimed `from typing import Any` is unused. Actually used on the
  next line as `JsonObject = dict[str, Any]` and as the return type
  of `api_get` / `http_get_json`. Reject.

---

## Aggregate

| owner | verified rows | unverified rows | rejected | est. lines |
| --- | --- | --- | --- | --- |
| lib/ | 6 | 5 | 2 | ~30-60 |
| modules/ | 4 (kept-tags excluded) | 0 | 7+ option blocks | ~15-30 |
| packages/ | 3 | 4 large files not covered | 4 | ~5-10 |
| site/ | 2 | 2 | 0 | ~70-90 (one whole file) |
| tests/ | 0 | 0 | many | 0 |
| tools/ | 0 | 1 cluster | 2 | ~50-100 if cluster pans out |

**Estimated upper-bound deletion (verified only):** ~170-290 lines.
**With unverified candidates, if all pan out:** ~400-700 lines.

The ceiling is lower than the prompt's ambition suggests. The repo is
already in reasonable shape: most "obvious" lib slop has been pulled
into the auto-discovered registries, lib helpers are usually called from
more than one place, and the modules' large option blocks ARE the
public surface — they look like slop but are not.

The biggest leverage left is in `packages/` (large Rust files I couldn't
cover in this pass) and in `tests/default.nix` (3700 lines that would
benefit from a focused category-11 read).

## Next step

Phase B: review per-row or per-category. After approval, Phase C
proceeds owner-by-owner with one PR per owner, `nix run .#lint` between
each batch.
