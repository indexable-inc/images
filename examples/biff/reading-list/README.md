# Reading List

Reading List is the smallest complete Biff 2 service on Index. Submit a link,
cross an explicit effect and authorization boundary, and persist it in SQLite
inside a private VM. The application stays in one namespace so the full request
path is visible before [Todo App](../todo-app/README.md) splits it into modules.

## Run

```sh
# From this directory (examples/biff/reading-list in the index repo).
ix apply .#biff-reading-list
ix port-forward biff-reading-list 8080:8080
```

Get the repo with `git clone https://github.com/indexable-inc/index`.

Open `http://127.0.0.1:8080`. The page shows a title-and-URL form; saved links
persist under `/var/lib/biff-reading-list`.

## Shape

- [`reading_list.clj`](src/com/example/reading_list.clj) contains the ordered
  modules, routes, SQLite schema, state transition, and authorization boundary.
- [`deps.edn`](deps.edn) declares the Clojure closure, while
  [`deps-lock.json`](deps-lock.json) makes it reproducible in Nix.
- [`service.nix`](service.nix) runs the package as a hardened non-root service,
  provides `sqlite3def` from Nix, persists state, and declares health checks.

Every split Biff library is pinned to revision
`b3abe5b13824af2f83f89ec31c63a430417ac457`, so the example evaluates against
one tested API surface.

## Verify

```sh
nix build .#default
nix shell nixpkgs#clojure nixpkgs#sqldef -c clojure -M:test
nix build .#checks.x86_64-linux.biff-reading-list-vm -o result-vm
ls result-vm
```

The unit tests exercise URL normalization and effect construction. The VM test
proves that rejected or unauthorized submissions cannot write, titles are HTML
escaped, duplicate URLs upsert, SQLite stays valid, and both data and the cookie
secret survive clean and forced service restarts. `result-vm` contains
`biff-reading-list.html` and `biff-reading-list.db`, never the secret.

The `.ix` output requires `ix` or Index's patched Nix with `wasm-builtin`; stock
`nix flake check` cannot evaluate it.

After changing dependencies:

```sh
nix run .#deps-lock
nix build .#default
```

This is deliberately single-user: `authorize-local-write` accepts every local
write. Replace it with identity-aware rules before sharing the app. Add
`biff.graph` or Litestream only when the query or recovery requirements justify
them.

## Next step

Continue with [Todo App](../todo-app/README.md) for authentication, live query
updates, background jobs, user isolation, and the admin surface. The
[parent guide](../README.md) compares both examples and states their shared
deployment boundary.
