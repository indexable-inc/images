# sqlmerge

A git merge driver for SQLite database files: a real three-way merge of row
data via the [SQLite session extension](https://sqlite.org/sessionintro.html),
instead of git's default "binary files conflict".

Built by Claude Code.

## How it works

git hands the driver three files: the common ancestor (`%O`), our version
(`%A`), and their version (`%B`). sqlmerge computes the changeset
`base -> theirs` with the session extension and applies it onto `ours` (in
place, as git expects), keyed by primary key:

- a row only one side changed merges cleanly;
- both sides making the same change (same edit, identical insert, or both
  deleting the row) is not a conflict;
- both sides changing the same row differently is a conflict, and so is a
  delete on one side against an edit on the other: the driver prints a
  per-row report (table, primary key, ours vs theirs values) to stderr and
  exits 1, so git marks the file conflicted;
- after a clean apply, `PRAGMA integrity_check` and `PRAGMA foreign_key_check`
  must pass, or the merge is refused.

## git wiring

In the ix base profile this is wired for every VM by home-manager
(`modules/profiles/base`): `*.db` / `*.sqlite` / `*.sqlite3` files merge with
sqlmerge, everything else keeps [mergiraf](https://mergiraf.org/). By hand:

```gitattributes
# .gitattributes
*.db merge=sqlite
```

```ini
# git config
[merge "sqlite"]
    name = SQLite three-way merge (sqlmerge)
    driver = sqlmerge %O %A %B
```

## Exit codes

| code | meaning                                                            |
| ---- | ------------------------------------------------------------------ |
| 0    | merged clean; `%A` now holds the merged database                   |
| 1    | conflict or refusal (details on stderr); git marks the file conflicted |

## Refusals (by design, no fallbacks)

- **Schema divergence.** If `sqlite_schema` differs between ours and theirs
  (ignoring whitespace and SQL comments outside quoted literals), the driver refuses and lists
  the differing objects. Changesets are data-only; sqlmerge never pretends to
  merge DDL. The merge base must share the schema too: even when both sides
  applied the same migration, the session diff cannot span a schema change,
  so that case is a (typed) refusal as well.
- **Missing primary key.** Any user table without an explicit `PRIMARY KEY` is
  a refusal naming the tables: the session extension silently skips such
  tables, which would be silent data loss.
- **Any row conflict aborts the whole merge.** v1 has no auto-resolution
  policies; the conflict handler is a policy enum so per-table policies
  (last-writer-wins, append-only) can be added later.

## Limitations

- No DDL merge: all three versions (base, ours, theirs) must share the schema.
- Every user table needs an explicit `PRIMARY KEY`.
- The database is treated as data at rest: WAL sidecar files are not
  considered (git never versions a live database anyway; checkpoint before
  committing).
