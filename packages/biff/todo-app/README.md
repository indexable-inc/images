# Todo App

What does [Reading List](../reading-list/README.md) look like once real users
show up? Todo App is the same write path with authentication, user-scoped
writes, background jobs, and Datastar live queries, spread over 16 namespaces.
Sign in with a code delivered to the service journal, open two tabs, and a
committed SQLite change reaches the second tab without a reload.

This application is adapted from Biff's MIT-licensed `v2.x` demo at revision
`b3abe5b13824af2f83f89ec31c63a430417ac457`; see [`LICENSE.biff`](LICENSE.biff).

## Build

```sh
nix build .#biff-todo-app
```

The package currently requires an `x86_64-linux` builder because its lock
contains the Linux brotli native classifier. The source and Tailwind asset are
also prepared for `aarch64-darwin`, but the package stays unavailable there
until the lock includes `native-osx-aarch64`.

From the repo root. Get the repo with
`git clone https://github.com/indexable-inc/index`.

Each of the 16 namespaces compiles to its own content-addressed derivation
([`lib/build/clj-unit.nix`](../../../lib/build/clj-unit.nix)) over a dependency
closure of one fetch derivation per artifact
([`lib/build/clj-lock.nix`](../../../lib/build/clj-lock.nix)). The
[parent guide](../README.md) explains that split.

## Run it

Todo App is a package plus a NixOS module, not a checked-in example. Enable the
module on a VM:

```js
index.lib.mkVm({
  name: "todo",
  modules: [{ services: { "biff-todo-app": { enable: true } } }],
});
```

Then forward the port and open the page:

```sh
ix port-forward todo 8080:8080
```

Enter any email address at `http://127.0.0.1:8080`. With no mail provider
configured the application prints the one-time code to stdout, which systemd
files in the journal:

```sh
ix shell todo -- journalctl -u biff-todo-app -n 30
```

Open `/app` in two tabs. Create or complete a todo in one and the other
updates on its own. The database and the cookie secret persist under
`/var/lib/biff-todo-app`.

## Module options

[`modules/services/biff-todo-app`](../../../modules/services/biff-todo-app/default.nix)
runs the application as the non-root `biff-todo-app` user under
`ProtectSystem = "strict"` with `UMask = "0077"`, generates the session cookie
secret into the state directory before the JVM starts, and puts `sqldef` on the
unit's PATH. Beyond `enable`, `package`, `port`, and `host`:

- `baseUrl` is the absolute URL sign-in links are built against. It defaults to
  `http://localhost:<port>`, so a deployment behind a real hostname has to set
  it or emailed links point at the wrong host.
- `secure` marks session cookies `Secure`. Off by default, because a `Secure`
  cookie is never sent back over the plain HTTP the demo is reached on and
  sign-in then fails silently.
- `skipCaptcha` is on by default: the application ships without Turnstile keys
  and would otherwise reject every sign-in.
- `environmentFile` defaults to `-/var/lib/biff-todo-app/config.env`, optional
  because of the leading `-`. Operator secrets that must stay out of the Nix
  store go there.

## Shape

- [`components.clj`](src/com/example/todo_app/components.clj) composes Biff's
  config, admin, SQLite, queue, and Jetty components.
- [`modules.clj`](src/com/example/todo_app/modules.clj) assembles the
  application modules, and [`routes.clj`](src/com/example/todo_app/routes.clj)
  names every path in one place.
- [`model/schema.clj`](src/com/example/todo_app/model/schema.clj) owns the
  SQLite columns and `authorize`, the one function every write passes through.
  It checks the session `uid` against each row's owner per table, so one user
  cannot touch another's todos or tab state.
- [`app/archive.clj`](src/com/example/todo_app/app/archive.clj) is the
  background side: a `biff.fx` transition partitions the user's active todos
  into batches and submits them to the `:todo/archive` queue.
- [`deps.edn`](deps.edn) declares the Clojure closure;
  [`deps-lock.json`](deps-lock.json) fixes it in Nix at 300 Maven artifacts and
  10 git libraries.
- [`default.nix`](default.nix) builds the package;
  [`package.nix`](package.nix) registers it, with `overlay = true` so the
  service module can resolve `pkgs.biff-todo-app` by name.

A Datastar POST enters a `biff.fx` transition and then `authorize`; the
committed SQLite transaction refreshes every open SSE query.

## Browser assets

[`pins.json`](pins.json) fixes the versions and hashes of the two assets the
page loads: Tailwind CSS 4.3.0 and Datastar 1.0.1. `default.nix` compiles the
CSS and installs `datastar.js` into the runtime classpath, so the deployed page
never reaches a CDN. Generated assets are kept out of the compile units, so
rebuilding CSS does not recompile a namespace.

Tailwind's standalone binary is pinned per platform, one entry per system:

```json
"platforms": {
  "aarch64-darwin": { "asset": "tailwindcss-macos-arm64", "hash": "..." },
  "x86_64-linux":   { "asset": "tailwindcss-linux-x64",   "hash": "..." }
}
```

Building on a system with no entry fails with a message naming the system and
this file, instead of falling back to a download. That binary is a Bun
single-file executable with its payload appended after the ELF image, so
`patchelf` corrupts it; `default.nix` invokes the untouched artifact through
the dynamic loader.

## Verify

```sh
nix shell nixpkgs#clojure nixpkgs#sqldef -c clojure -M:test
```

Run from this directory. Dependencies come from Maven Central and Clojars
rather than from the Nix closure, so the first run needs network access. The
suite boots a real Jetty on a loopback port and drives it over HTTP: sign-in
and the admin page, todo mutations, the selected Datastar filter styling, the
archive queue, cross-user isolation, title validation, and the admin alert
mailer in the initialized system context. It reports 11 tests and 71 assertions.
Run on aarch64-darwin at 0 failures and 0 errors.

`clojure` leaves a `.cpcache/` directory behind; delete it afterwards.

[`vm-test.nix`](vm-test.nix) boots the real package under the shipped module
and drives sign-in, SQLite, a two-tab Datastar update in headless Firefox
([`browser-test.py`](browser-test.py)), and recovery from a clean stop and a
SIGKILL:

```sh
nix build .#checks.x86_64-linux.biff-todo-app-vm -o result-vm
```

It needs an `x86_64-linux` builder. `result-vm` holds the exported page, the
SQLite database, and the Firefox screenshot; it never holds the cookie secret.
The check is its own attribute rather than part of the `eval` aggregate because
it boots a qemu VM.

After changing `deps.edn`:

```sh
nix run github:jlesquembre/clj-nix#deps-lock
nix build .#biff-todo-app
```

## Security boundary

The checked-in configuration is a zero-configuration private demo: HTTP, no
captcha, and sign-in mail printed to the service journal. Do not expose it
publicly in that mode. A public deployment sets an HTTPS `baseUrl`, turns
`secure` on, turns `skipCaptcha` off with real `TURNSTILE_SITE_KEY` and
`TURNSTILE_SECRET_KEY` values, and supplies `MAILERSEND_API_KEY`,
`MAILERSEND_FROM`, and `MAILERSEND_REPLY_TO` through `environmentFile` at mode
`0400`.

State lives on one VM. For the smallest version of this request path, start
with [Reading List](../reading-list/README.md); the
[parent guide](../README.md) covers both applications.
