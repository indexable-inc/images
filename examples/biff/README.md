# Biff 2 on Index

These examples show one Biff 2 application path at two sizes. Reading List keeps
all application behavior in one namespace. Todo App splits the same path into
modules for authentication, user-scoped writes, live queries, and background
work. Both package with `clj-nix`, run as hardened non-root systemd services,
and keep SQLite state on the VM.

## Choose an example

Start with [Reading List](reading-list/README.md) for the smallest complete
form, effect, authorization, and SQLite path. Continue with
[Todo App](todo-app/README.md) for authentication, Datastar updates, background
jobs, and user isolation. Todo App adds application boundaries without changing
the deployment model.

## Run

Apply either example from its own directory:

```sh
# From examples/biff/reading-list in the index repo.
ix apply .#biff-reading-list
ix port-forward biff-reading-list 8080:8080
```

```sh
# From examples/biff/todo-app in the index repo.
ix apply .#biff-todo-app
ix port-forward biff-todo-app 8080:8080
```

Get the repo with `git clone https://github.com/indexable-inc/index`.

Open `http://127.0.0.1:8080`. Reading List accepts a title and URL. Todo App
sends its sign-in code to the service journal:

```sh
ix shell biff-todo-app -- journalctl -u biff-todo-app -n 30
```

## Request path

Both examples keep the write boundary visible:

```text
browser -> route -> biff.fx transition -> authorization -> SQLite transaction
```

Todo App continues the path after commit:

```text
SQLite transaction -> live query refresh -> Datastar SSE update
```

The source tree follows the same split:

- `src/` contains ordinary Clojure application code.
- `deps.edn` declares dependencies; `deps-lock.json` fixes the Nix closure.
- `service.nix` owns users, systemd hardening, health checks, and persistent
  state.
- `default.ix` declares the VM; `flake.nix` exposes the package and system test.
- `test/` checks application behavior; the NixOS VM checks exercise the packaged
  service and its recovery behavior.

Every split Biff library is pinned to revision
`b3abe5b13824af2f83f89ec31c63a430417ac457`. Todo App also vendors its browser
assets through fixed-output Nix inputs, so the deployed page does not depend on
a CDN.

## Verify

Run each command from the example directory:

```sh
nix build .#default
nix shell nixpkgs#clojure nixpkgs#sqldef -c clojure -M:test
```

Then build the corresponding system check:

```sh
# Reading List
nix build .#checks.x86_64-linux.biff-reading-list-vm -o result-vm

# Todo App
nix build .#checks.x86_64-linux.biff-todo-app-vm -o result-vm
```

The Reading List check covers invalid and unauthorized writes, HTML escaping,
URL upserts, SQLite integrity, and state recovery after clean and forced
restarts. The Todo App check adds sign-in, CSRF, user isolation, queued archive
jobs, local browser assets, and a real two-tab Datastar update in Firefox.
Neither check exports the cookie secret or sign-in code.

The `.ix` entrypoints require `ix` or Index's patched Nix with `wasm-builtin`.
The NixOS VM checks require an `x86_64-linux` builder. Each `result-vm`
directory contains the exported HTML, SQLite database, and, for Todo App, the
Firefox screenshot described in the example README.

## Deployment boundary

The checked-in deployments are private examples, not public service defaults.
Reading List authorizes every local write. Todo App uses HTTP, skips captcha,
and prints sign-in mail to the journal. Before exposing either application,
add identity-aware authorization where needed. For Todo App, also configure
HTTPS, secure cookies, captcha, and a real mail provider.

Both examples keep state on one VM. Add replication or backup only when the
recovery requirement calls for it; neither example claims high availability.
