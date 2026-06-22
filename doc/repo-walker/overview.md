# repo-walker

`packages/repo-walker` walks a directory tree the way a source-code consumer
wants: honor `.gitignore` (plus global, exclude, and `.ignore` files), skip
hidden entries, skip known binary extensions, and yield the remaining files
through a fallible iterator. It wraps [`ignore::WalkBuilder`](https://docs.rs/ignore)
and adds the binary-extension filter and error surfacing so callers do not
reimplement them. Single Rust workspace library crate (`id = repo-walker`, no
flake output); consumers are the search/corpus tooling
(`packages/search/file-search/Cargo.toml`, `packages/search/search-core/Cargo.toml`).

## Public surface (`src/lib.rs`)

- `WalkOptions { respect_gitignore, follow_links }` (`lib.rs:16-33`): defaults
  `respect_gitignore = true`, `follow_links = false`. Turning off
  `respect_gitignore` silences *all* ignore sources (gitignore, global, exclude,
  `.ignore`, parents, hidden), not just git ones (`lib.rs:43-56`).
- `FileScanner::new(&Path, WalkOptions)` (`lib.rs:42`): an `Iterator<Item =
  Result<PathBuf, WalkError>>` (`lib.rs:65-95`). It yields only regular files
  (resolved without following symlinks unless `follow_links`), drops paths whose
  extension is on the binary list, and surfaces walk errors as `Err` items
  rather than dropping them silently. `WalkError` is re-exported `ignore::Error`
  (`lib.rs:14`).
- `GitignoreFilter::new(&Path, respect_gitignore)` + `filter_paths(Vec<PathBuf>)
  -> Vec<PathBuf>` (`lib.rs:103-168`): apply a standalone gitignore matcher to a
  path list that came from somewhere other than the walker (a watcher, a diff, a
  manifest). It loads only the root `.gitignore`; for nested ignores use
  `FileScanner`. Uses `matched_path_or_any_parents` so a directory rule like
  `target/` excludes `target/debug/foo.rs` (`lib.rs:157-164`).
- `is_indexable_file(&Path) -> bool` (`lib.rs:193`): regular file (via
  `symlink_metadata`, so symlinks are excluded) whose extension is not binary.
  No extension counts as text.
- `is_binary_extension(&str) -> bool` (`lib.rs:210`): the case-insensitive
  known-binary extension list (executables, objects, images, audio/video,
  archives, office docs, wasm/jvm/python bytecode).

No flake package, no CLI: library only.
