# Todo App

Todo App continues [Reading List](../reading-list/README.md) with
authentication, user-scoped writes, background jobs, and live query updates.
Sign in with a console-delivered code, open two tabs, and watch a committed
SQLite change reach the second tab through Datastar. The module boundaries keep
that request path traceable from browser event to authorized transaction.

## Run

```sh
# From this directory (examples/biff/todo-app in the index repo).
ix apply .#biff-todo-app
ix port-forward biff-todo-app 8080:8080
```

Get the repo with `git clone https://github.com/indexable-inc/index`.

Open `http://127.0.0.1:8080` and enter any email address. This private example
uses Biff's console mailer; read the one-time code with:

```sh
ix shell biff-todo-app -- journalctl -u biff-todo-app -n 30
```

Open `/app` in two tabs. Create or complete a todo in one; the other updates
without a reload. The database and cookie secret persist under
`/var/lib/biff-todo-app`.

## Shape

- [`components.clj`](src/com/example/todo_app/components.clj) composes Biff's
  configuration, admin, SQLite, queue, and Jetty components.
- [`modules.clj`](src/com/example/todo_app/modules.clj) assembles the application
  modules; [`schema.clj`](src/com/example/todo_app/model/schema.clj) owns the
  schema and write-authorization boundary.
- [`service.nix`](service.nix) runs a hardened non-root service, generates a
  persistent cookie secret, and provides `sqlite3def` from Nix.
- [`flake.nix`](flake.nix) fixes Clojure and browser dependencies, compiles
  Tailwind, and exposes the package and VM test.

A Datastar POST enters a `biff.fx` transition and the central authorization
function; the committed SQLite transaction then refreshes every open SSE query.

Every split Biff library is pinned to revision
`b3abe5b13824af2f83f89ec31c63a430417ac457`, so the example evaluates against
one tested API surface. This app is adapted from Biff's MIT-licensed `v2.x`
demo at that revision; see [`LICENSE.biff`](LICENSE.biff).

## Verify

```sh
nix build .#default
nix shell nixpkgs#clojure nixpkgs#sqldef -c clojure -M:test
nix build .#checks.x86_64-linux.biff-todo-app-vm -o result-vm
ls result-vm
```

The unit tests exercise authentication, CSRF, graph and effects composition,
user isolation, validation, and queued archive jobs. The VM test also proves
that browser assets are local, a real Firefox session updates a second tab, one
user cannot mutate another's data, SQLite stays valid, and state survives clean
and forced service restarts. `result-vm` contains
`biff-todo-app.html`, `biff-todo-app.db`, and `biff-todo-app.png`, never the
cookie secret or a sign-in code.

The `.ix` output requires `ix` or Index's patched Nix with `wasm-builtin`; stock
`nix flake check` cannot evaluate it.

After changing dependencies:

```sh
nix run .#deps-lock
nix build .#default
```

## Security boundary

The checked-in deployment is deliberately a zero-configuration, private demo:
it uses HTTP, skips captcha, and prints sign-in mail to the service journal. Do
not expose it publicly in this mode. A public deployment must set an HTTPS
`BASE_URL`, enable secure cookies and captcha, and provide MailerSend credentials
through `/var/lib/biff-todo-app/config.env` with mode `0400`.

For the smallest request path, start with
[Reading List](../reading-list/README.md). The [parent guide](../README.md)
compares both examples and lists their shared verification commands.
