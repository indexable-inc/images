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

Generated catalogs are build inputs, not hand-edited source. If a generated file
is wrong, change the manifest or generator that owns it.

Keep binary and generated artifacts near the owner that can explain and refresh
them. Use small manifests for curated sets, generated catalogs for URLs and
hashes, and metadata catalogs for search or browsing surfaces.

Repository examples should consume those shared surfaces. Repeating URLs and
hashes in examples creates second owners with no update story.

