## Dependency intake

Every external input needs an owner that can update it predictably. Prefer
ecosystem lockfiles, flake inputs for real flake-level tools, repo manifests
consumed by updaters, or narrow `pkgs.*` fetchers when no better owner exists.

The human workflow is: edit the source requirement or manifest, run the owning
updater, inspect the generated diff, and commit the source and generated
hash-bearing output together.

Use the most specific `pkgs.*` fetcher for the source: `fetchurl` for opaque
single files, forge fetchers for forge snapshots, `fetchgit` for raw git refs,
`fetchzip` for archives that must unpack, and ecosystem fetchers when one
exists. Avoid `builtins.fetch*` in tracked Nix files because those fetch during
eval and do not substitute like fixed-output derivations.

Tracked Nix files should never contain fake hash helpers or placeholder hashes.
Materialize real SRI hashes with the owning updater, lock command,
`nix flake update`, or a checked prefetch command before committing.

Use `__impure` only for explicit dependency-discovery or prefetch derivations
that are turned into a checked hash-bearing artifact before product builds
consume them. Keep the impure boundary named next to the updater or generated
lock output that makes later builds pure.

Generated catalogs are build inputs, not hand-edited source. If a generated file
is wrong, change the manifest or generator that owns it.

A prebuilt-binary package pins its version and per-platform hashes in a generated
`manifest.json` read with `lib.importJSON` and refreshed by a
`passthru.updateScript`; bump by running the updater, never by hand-editing the
hashes. When upstream signs its release manifest, the updater verifies that
signature against a pinned key and fails closed before writing hashes. See
[`packages/claude-code`](packages/claude-code) for the worked shape:
`nix run .#claude-code.updateScript -- <version>`.

Keep binary and generated artifacts near the owner that can explain and refresh
them. Use small manifests for curated sets, generated catalogs for URLs and
hashes, and metadata catalogs for search or browsing surfaces.

Repository examples should consume those shared surfaces. Repeating URLs and
hashes in examples creates second owners with no update story.
