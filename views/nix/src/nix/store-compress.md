R""(

# Examples

* Report how much a full sweep of the store would save, without changing
  anything:

  ```console
  # nix store compress --all --dry-run
  ```

* Compress every path in the store:

  ```console
  # nix store compress --all
  ```

* Compress the closure of the current system profile:

  ```console
  # nix store compress --recursive /run/current-system
  ```

# Description

This command applies macOS transparent (decmpfs) compression, using LZFSE,
to store paths that are **already** in the store — the after-the-fact
counterpart of the `compress-store-paths` setting, which only compresses
paths as they are added.

The kernel decompresses on read, so file contents — and therefore NAR
hashes and store paths — are unaffected; only the on-disk footprint
shrinks. Files that would not save at least one allocation block are left
alone, and every compressed file is read back and compared before the
compression is accepted.

The sweep is idempotent and may be interrupted at any time: files that are
already compressed carry the `UF_COMPRESSED` flag and are skipped cheaply,
so re-running only does the remaining work.

Hard-linked files (those deduplicated by `nix store optimise` or
`auto-optimise-store`) are never touched, since rewriting one name of a
shared inode would rewrite them all. They are counted and reported
separately. Files deduplicated with APFS **clones** (`clone-store-paths`)
cannot be detected, and compressing one **breaks its extent sharing**,
which can make total disk usage go *up*; do not run this on a
clone-optimised store.

Compression rewrites file metadata, so it needs write access to the store:
run it as root (or against a store you own with
`--store 'local?root=...'`).

This command is only available on macOS, on APFS (or HFS+) volumes.

)""
