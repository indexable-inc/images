# Reading List

How small can a complete Biff 2 service get? This one is a single Clojure
namespace: a title-and-URL form, one effect, one authorization function, one
SQLite table. The whole request path fits in
[`reading_list.clj`](src/com/example/reading_list.clj), which is the point.
[Todo App](../todo-app/README.md) is the same path split across 16 namespaces.

## Build

```sh
nix build .#biff-reading-list
```

From the repo root. Get the repo with
`git clone https://github.com/indexable-inc/index`.

The build compiles the one namespace as a content-addressed unit
([`lib/build/clj-unit.nix`](../../../lib/build/clj-unit.nix)) over a dependency
closure of one fetch derivation per artifact
([`lib/build/clj-lock.nix`](../../../lib/build/clj-lock.nix)). The
[parent guide](../README.md) explains that split.

## Run it

```sh
# From examples/biff/reading-list in the index repo.
ix apply .#biff-reading-list
ix port-forward biff-reading-list 8080:8080
```

Open `http://127.0.0.1:8080`. The page is a title-and-URL form; saved links
land in `/var/lib/biff-reading-list/reading-list.db`. That state directory is
the only writable path the unit has.

To put the service on a VM of your own instead, enable the module:

```js
index.lib.mkVm({
  name: "links",
  modules: [{ services: { "biff-reading-list": { enable: true } } }],
});
```

[`modules/services/biff-reading-list`](../../../modules/services/biff-reading-list/default.nix)
takes `port` and `host` alongside `enable` and `package`, runs the application
as the non-root `biff` user under `ProtectSystem = "strict"`, generates the
session cookie secret into the state directory before the JVM starts, and puts
`sqldef` on the unit's PATH so the schema is applied without a download.

## Shape

- [`reading_list.clj`](src/com/example/reading_list.clj) holds the modules,
  routes, SQLite columns, state transition, and authorization boundary.
- [`deps.edn`](deps.edn) declares the Clojure closure;
  [`deps-lock.json`](deps-lock.json) fixes it in Nix at 253 Maven artifacts and
  4 git libraries.
- [`default.nix`](default.nix) builds the package;
  [`package.nix`](package.nix) registers it, with `overlay = true` so the
  service module can resolve `pkgs.biff-reading-list` by name.

Every Biff library is pinned to revision
`b3abe5b13824af2f83f89ec31c63a430417ac457`, so the application evaluates
against one tested API surface.

## The write boundary

`authorize-local-write` accepts every local write, and that is deliberate:
this is a single-user example, and that one function is the place identity-aware
rules go when an authentication module arrives. Replace it before sharing the
application. Reach for `biff.graph` or Litestream only when the query or
recovery requirement justifies them.

## Verify

```sh
nix shell nixpkgs#clojure nixpkgs#sqldef -c clojure -M:test
```

Run from this directory. Dependencies come from Maven Central and Clojars
rather than from the Nix closure, so the first run needs network access. It
reports 5 tests and 32 assertions, covering input normalization, the links
query, URL validation, the create-link state transition, and the cookie-secret
contract. Run on aarch64-darwin at 0 failures and 0 errors.

`clojure` leaves a `.cpcache/` directory behind; delete it afterwards.

After changing `deps.edn`:

```sh
nix run github:jlesquembre/clj-nix#deps-lock
nix build .#biff-reading-list
```

## Next step

[Todo App](../todo-app/README.md) adds authentication, live query updates,
background jobs, user isolation, and an admin surface. The
[parent guide](../README.md) covers both applications and the deployment
boundary they share.
