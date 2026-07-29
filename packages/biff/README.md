# Biff 2 on Index

What does a [Biff 2](https://biffweb.com/) application look like when Nix owns
the build and systemd owns the deployment? Two of them live here, at two sizes.
Reading List keeps the whole path from HTML form to SQLite row in one
namespace. Todo App splits that same path across 16 namespaces for
authentication, user-scoped writes, live queries, and background work. Both are
ordinary repo packages; neither carries a flake of its own.

## Pick one

[Reading List](reading-list/README.md) is the smaller: a title-and-URL form,
one authorization function, one SQLite table.
[Todo App](todo-app/README.md) adds sign-in, per-user data, a job queue, and
Datastar updates pushed over SSE. The application boundaries differ. The
deployment model does not.

## Build

```sh
nix build .#biff-reading-list
nix build .#biff-todo-app
```

Both run from the repo root. Get the repo with
`git clone https://github.com/indexable-inc/index`.

[`packages/registry.nix`](../registry.nix) discovers each application from its
`package.nix`, so CI builds them like every other package here. Each output is
a launcher script that execs `java -cp <closure> com.example.<app>`.

## How the build is split

Two builders do the work, and both are shaped so that a small edit costs a
small rebuild.

[`lib/build/clj-lock.nix`](../../lib/build/clj-lock.nix) turns `deps-lock.json`
into a classpath, one fetch derivation per artifact. Reading List's lock names
253 Maven artifacts and 4 git libraries; Todo App's names 300 and 10. Bumping
one library re-runs that library's fetch and leaves the rest alone. The lock is
written in clj-nix's schema (`lock-version` 4) and regenerated with
`nix run github:jlesquembre/clj-nix#deps-lock`; nothing in the build itself
uses clj-nix.

[`lib/build/clj-unit.nix`](../../lib/build/clj-unit.nix) compiles one
content-addressed derivation per namespace. The edges come from each file's
`ns` form, transcribed to JSON by the Rust renderer
[`packages/nix-clj-unit`](../nix-clj-unit). It is the Clojure counterpart of
[`lib/rust/cargo-unit.nix`](../../lib/rust/cargo-unit.nix), which is one
derivation per rustc invocation, and
[`lib/kernel/kbuild-unit.nix`](../../lib/kernel/kbuild-unit.nix), one per C
translation unit. `passthru.units` names every namespace, so a single namespace
can be built on its own.

One rule falls out of that split and is easy to trip over: a unit sees its
dependencies' compiled classes and never their `.clj` files. Clojure's loader
prefers a `.class` over a `.clj` only when the class file's mtime is strictly
greater, and Nix normalizes every store mtime to the same second, so a visible
dependency source ties, loses, and gets recompiled into the wrong unit's
output. The header comment in `clj-unit.nix` has the measurements.

## Deploy

Each application has an auto-discovered NixOS module:

- [`modules/services/biff-reading-list`](../../modules/services/biff-reading-list/default.nix)
- [`modules/services/biff-todo-app`](../../modules/services/biff-todo-app/default.nix)

Enable one in a VM:

```js
index.lib.mkVm({
  name: "biff-reading-list",
  modules: [{ services: { "biff-reading-list": { enable: true } } }],
});
```

[`examples/biff/reading-list`](../../examples/biff/reading-list) is that VM,
checked in:

```sh
# From examples/biff/reading-list in the index repo.
ix apply .#biff-reading-list
ix port-forward biff-reading-list 8080:8080
```

Todo App has no example of its own. Enable `services.biff-todo-app` in a VM you
already have.

Both modules create the shared `biff` system user, apply `ix.systemdHardening`
with `ProtectSystem = "strict"`, generate the session cookie secret into the
state directory before the JVM starts, and put `sqldef` on the unit's PATH so
schema migration never fetches a binary at runtime. State lives in
`/var/lib/biff-reading-list` and `/var/lib/biff-todo-app`, the only writable
paths the units have.

## Request path

Both applications keep the write boundary visible:

```text
browser -> route -> biff.fx transition -> authorization -> SQLite transaction
```

Todo App continues the path after commit:

```text
SQLite transaction -> live query refresh -> Datastar SSE update
```

Each package directory has the same layout:

- `src/` is ordinary Clojure. Nothing in it knows about Nix.
- `deps.edn` declares dependencies; `deps-lock.json` fixes the closure.
- `default.nix` builds the package. `package.nix` is its registry metadata,
  including `overlay = true`, which is what lets the service module resolve the
  application out of `pkgs` by name instead of taking it as an argument.
- `test/` holds `clojure.test` namespaces, reached through the `:test` alias in
  `deps.edn`.

Every Biff library in both applications is pinned to revision
`b3abe5b13824af2f83f89ec31c63a430417ac457`, so both evaluate against one tested
API surface. Todo App also vendors its browser assets through fixed-output
derivations, so the deployed page does not call out to a CDN.

## Verify

Run the unit tests from either package directory:

```sh
nix shell nixpkgs#clojure nixpkgs#sqldef -c clojure -M:test
```

That resolves dependencies from Maven Central and Clojars rather than from the
Nix closure, so the first run needs network access and takes a few minutes.
Reading List reports 5 tests and 32 assertions. Todo App boots a real Jetty on
a loopback port and reports 11 tests and 69 assertions. Both were run this way
on aarch64-darwin at 0 failures and 0 errors.

`clojure` writes a `.cpcache/` directory into the package it runs in. Delete it
afterwards; no `.gitignore` covers it here yet.

## Deployment boundary

The checked-in deployments are private examples, not public service defaults.
Reading List authorizes every local write. Todo App runs over HTTP, skips
captcha, and prints sign-in mail to the service journal. Before exposing either
application, add identity-aware authorization where it is needed. For Todo App,
also set an HTTPS `baseUrl`, turn `secure` on, turn `skipCaptcha` off, and
supply a real mail provider through `environmentFile`.

Both applications keep state on one VM. Add replication or backup when a
recovery requirement calls for it; neither claims high availability.
