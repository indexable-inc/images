# mirror

Opt-in standalone GitHub repos, generated from this monorepo. Two products,
one source-generation core:

- **Package mirrors**: a package under `packages/` opts in via its
  `package.nix` and gets a self-contained, read-only GitHub repo that a
  visitor can clone and `cargo build` without ever seeing the monorepo. The
  monorepo stays the single source of truth; CI keeps the mirror equal to it.
- **Fork branches**: a de-forked package (lib/fork-packages.nix) can opt into
  a real GitHub fork repo whose `ix-patched` branch is defined declaratively —
  the upstream base pinned in `flake.lock` plus the in-repo `patches/` series
  applied as commits — so opening an upstream PR is one push away.

Rust packages are what the generator understands today, but the interface
(the `mirror` manifest attr, `.#lib.mirrorPackages`, the sync workflow) is
language-neutral by design; another ecosystem adds a generator, not a new
pipeline.

## How a mirror is made

`mirror gen --package packages/<path> --out <dir>` produces the standalone
tree:

- The crate's own files sit at the output root. Its intra-workspace
  dependency closure (computed from the root manifest's
  `[workspace.dependencies]` path entries) goes under `crates/<name>/`, and a
  `[workspace]` table stitches them together; a true leaf crate stays a plain
  single-crate repo.
- Every `Cargo.toml` is rewritten standalone: `version.workspace = true` and
  friends become concrete values, `dep.workspace = true` becomes the concrete
  version with member feature additions merged (cargo's own inheritance
  semantics), internal deps become `path = "crates/<name>"`, `publish =
  false` is pinned, `license = "MIT"` is injected (the root LICENSE rides
  along), and the `[lints]` table is dropped — the workspace lint set names
  lints only the org's patched clippy knows. Comments survive; the rewrite is
  format-preserving.
- The `Cargo.lock` is **pruned, never re-resolved**: the subset of the root
  lock reachable from the mirrored crates, so a mirror builds against exactly
  the versions the monorepo builds against. `rust-toolchain.toml` is copied
  for the same reason. One nuance: the monorepo lock records feature-union
  dependency edges (an optional dep activated by *any* workspace member shows
  up on the shared entry), so the pruned lock is a version-exact **superset**
  — a mirror's first `cargo build` may drop entries the mirrored crate never
  activates, but it never changes a version and never touches the network for
  resolution.
- The README leads with a banner naming the mirror a read-only generated
  artifact, linking the exact monorepo tree (path + commit) it came from and
  pointing issues/PRs at the monorepo. Below it: the package's own README
  when it has one, else a minimal generated body from the crate metadata.

`mirror publish --package packages/<path> [--create]` runs `gen` into a
scratch directory, clones the mirror repo, swaps its working tree for the
generated one, and commits **only when the tree actually changed**, as
`sync: indexable-inc/index@<sha>` with a `Source-Commit: <sha>` trailer, then
pushes `main`. Snapshot sync: one mirror commit per effective change, no
history filtering, cheap enough to run on every push to main. `--create`
creates the GitHub repo via `gh repo create` when it does not exist yet.

`mirror fork-branch --name <fork> [--push]` reads the fork mapping
(`.#lib.forkPackages`, or `--mapping <json>`), fetches the upstream repo at
the rev `flake.lock` pins for the fork's input, applies the `patches/` series
with `git am --3way` onto branch `ix-patched`, and — with `--push` — force
pushes that branch to the entry's `forkRepo`. Without `--push` it is a pure
verification that the series still applies. The branch is regenerated, never
merged into, so it is always a clean, properly rebased serialization of the
patch DAG.

## Adding a mirror

1. Add the attr to the package's `package.nix`:

   ```nix
   mirror.repo = "indexable-inc/<name>";
   # optional:
   # mirror.description = "One-line GitHub repo description";
   # mirror.topics = ["rust" "cli"];
   ```

   `packages/registry.nix` validates the keys; the entry surfaces in
   `nix eval --json '.#lib.mirrorPackages'`.

2. That's it. The mirror-sync workflow (`.github/workflows/mirror-sync.yml`)
   publishes on the next push to `main` touching `packages/**` (plus a daily
   cron and `workflow_dispatch`), creating the repo on first run.

To maintain a fork repo for a de-forked package instead, add
`forkRepo = "indexable-inc/<name>";` to its entry in lib/fork-packages.nix.

## Permissions

The default `GITHUB_TOKEN` can neither create repositories nor push to any
repo other than this one, so mirror-sync needs a `MIRROR_TOKEN` secret on
this repository. Two ways to mint it:

- **Recommended: an org-owned GitHub App**, installed on the `indexable-inc`
  org with
  - Administration: **write** (create the mirror/fork repos on first publish),
  - Contents: **write** (push `main` / `ix-patched`),
  - Metadata: **read** (implicit baseline).

  Store its id/private key as `APP_ID` / `APP_PRIVATE_KEY` secrets and mint a
  short-lived installation token per run with
  `actions/create-github-app-token`, feeding the output into `MIRROR_TOKEN`.
  Scoped, revocable, and not tied to a person's account.

- **Simpler: a PAT.** Fine-grained, org-owned, all-repositories (new mirror
  repos must fall inside its scope) with Administration + Contents write; or
  a classic PAT with the `repo` scope (classic PATs can create org repos,
  fine-grained repo *creation* otherwise needs the App route). Stored
  directly as `MIRROR_TOKEN`.

If repos are pre-created by hand, Administration/creation rights can be
dropped and `--create` becomes a no-op safety valve; Contents: write is the
floor.
