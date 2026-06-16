# nix-cargo-unit internals

The non-obvious mechanisms behind [overview](overview.md): unit identity hashing,
graph merge, source scoping, and the panic-reachability scan.

## Unit identity hash (`src/model.rs:612`)

Each unit's identity is a SHA-256 (first 8 bytes, 16 hex chars; `src/hash.rs:18`)
over a canonical encoding of everything that affects its compiled output, plus
its dependencies' hashes. `write_unit_identity` (`src/model.rs:638`) folds in:
package identity, target name and edition, sorted crate types, sorted features,
profile name/opt-level, and identity bytes for `lto`/`debuginfo`/`panic`/`strip`,
the boolean profile flags, codegen units, split-debuginfo, profile `rustflags`,
lint rustflags, check-cfg args, and the unit mode.

The dependency hashes are appended sorted, each as `dep\0<crate>:<public>:
<noprelude>:<hash>\0` (`src/model.rs:620`), so the identity is recursive: a
change deep in the graph propagates up to every dependent's hash but leaves
unrelated units untouched. An optional `--toolchain-id` is folded in last
(`src/model.rs:628`), so a toolchain bump invalidates the whole graph.

Sorting before hashing is what makes the output deterministic regardless of the
order Cargo emitted dependencies or features. The hash is the `<hash>` segment of
every unit name and feeds rustc's `-C metadata`, so two units that would compile
identically share an identity (and a store path under content-addressing).

## Package id parsing (`src/model.rs:479`)

`parse_pkg_id` handles both the legacy `<name> <version> (<source>)` form and the
modern `path+file://...`, `registry+...`, `git+...`, `sparse+...#name@version`
forms, extracting name and version (and stripping a trailing `.git`). `is_external`
(`src/model.rs:603`) classifies a unit as third-party by its source scheme; the
renderer uses this to skip clippy, the panic scan, and audit on vendored code.

## Merge (`src/model.rs:298`)

`UnitGraph::merge` unions several graphs into one. It first computes every unit's
identity hash within each graph (`merge_identity_hash`, recursively, with
memoization), then walks each unit, rewriting its dependency indices into the
merged graph and deduplicating by hash through `merged_by_hash`: two units with
the same identity across input graphs become one node. Each input graph's roots
are recorded both in the flat `roots` and as a `root_sets` entry, so a consumer
can tell which target each root came from. `validate` (`src/model.rs:353`) checks
every root and dependency index is in range before and after merge.

## Source scoping (`src/render.rs`)

A unit's source is emitted as one `sources.<name>` entry, not the whole tree, so
editing one crate does not invalidate its siblings. `SourceEntry`
(`src/render.rs:171`) carries a `SourceBase` and `SourceScope`:

- `SourceBase` (`src/render.rs:182`): `Workspace` / `WorkspaceClosure` /
  `VendorPackage` / `VendorClosure` selects which Nix helper builds the source
  (`scopedWorkspaceSource`, `scopedWorkspaceClosureSource`,
  `vendorSources.<key>`, `scopedVendorClosureSource`; rendered in
  `nix_expr`, `src/render.rs:241`).
- `SourceScope` (`src/render.rs:190`): `Package` (one crate dir) vs `Closure` (a
  crate plus the sibling paths it includes, for a workspace member that reaches
  outside its own dir).

The template's `scopedWorkspaceSource` builds a package-shaped `builtins.path`
rooted at `workspaceRoot/<relative>` (`units.nix.askama:105`), and the closure
variants use a `filter` that keeps only the include set
(`sourceClosureFilter`, `units.nix.askama:90`). A vendor closure asserts
`vendorDir != null` (`units.nix.askama:121`). The external-source map is built
from `Cargo.lock` (`CargoLockSources::from_path`, `src/render.rs:73`);
`source_for_unit` (`src/render.rs:102`) matches a unit's package id against the
lock to pick the exact source, erroring on a missing or ambiguous match so a
mis-resolved vendor path fails loudly. `cargo_lock_source_matches_pkg_id`
(`src/render.rs:146`) tolerates the git rev suffix the lock carries.

## Build-script handling

A package's `build.rs` appears as two units: a `custom-build` compile unit (real
workspace Rust, so it keeps clippy) and a `run-custom-build` unit that executes
the script. `prepare_graph` folds these into a `BuildScriptRun`
(`src/render.rs:166`) so the run unit depends on the compile unit and its own
dependency runs, and the renderer emits the run unit before the rustc units
(`render_unit_entries`, `src/render.rs:327`). The run unit is named
`<pkg>-build-script-run-<version>-<hash>` (`src/render.rs:582`).

## Panic-freedom scan (`src/panic_scan.rs`)

`scan-panics` is a relocation-based reachability check, not a disassembler. A
function that can panic emits a call to a `core::panicking::*` entrypoint; in a
relocatable object that call survives as a relocation targeting the undefined
panic symbol, at an offset inside the calling function's text range. The scanner
reads symbols and relocations with the `object` crate and attributes each panic
relocation to its containing function (`scan_object`, `src/panic_scan.rs:125`),
so the same logic covers ELF and Mach-O.

Key points:

- **Operates on `.o`/`.rlib`, not linked binaries** (`src/panic_scan.rs:11`): a
  linked binary resolves panic calls to direct branches with no relocation left
  to read.
- **Monomorphization attribution** (`src/panic_scan.rs:14`): a library generic is
  codegened where it is instantiated, so a panic in it carries the library's
  crate token in the consumer's object. Scanning every production unit and scoping
  findings to the whole workspace crate set
  (`workspace_crate_names`, `src/render.rs:446`) attributes it back to the
  defining crate. `crate_token` (`src/panic_scan.rs:78`) is the length-prefixed,
  dash-normalized crate name shared by legacy and v0 mangling.
- **Panic sinks** (`PANIC_SINKS`, `src/panic_scan.rs:202`): `core::panicking` and
  `std::panicking` plus the `unwrap_failed`/`expect_failed` cold paths that
  `unwrap`/`expect` route through. `at_crate_boundary` (`src/panic_scan.rs:222`)
  matches only when `core`/`std` is the crate root, so a user `crate::core::
  panicking` module does not trip it.
- **Fail closed** (`src/main.rs:144`, `src/panic_scan.rs:107`): finding no
  artifacts to scan, or an artifact that is neither an archive nor a parseable
  object, is an error, not a pass.
- **Scope** (`is_panic_freedom_candidate`, `src/render.rs:433`): only non-external,
  non-proc-macro, non-build-script, non-test, non-bench units are scanned: test
  and bench bodies legitimately panic. The rendered check
  (`render_panic_freedom_check`, `src/render.rs:463`) builds one scan derivation
  per candidate unit (so a touched unit re-scans only itself), joined under one
  aggregate, and asserts the `cargoUnit` scanner package is non-null.

This is a best-effort detector, not a soundness proof: a generic no production
unit instantiates, or a panic through an uncatalogued std cold path, is missed by
construction (`src/panic_scan.rs:21`). The sound successor is whole-binary
call-graph reachability.
