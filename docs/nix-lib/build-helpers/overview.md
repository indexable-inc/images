# lib/build: non-Rust build helpers

`lib/build/` holds the builders for everything that is not the
[cargo-unit Rust path](../rust/overview.md): JS/TS sites, Python uv apps, Go
units, Gradle fat-jars, Zig packages, and the libghostty-vt C library, plus the
lockfile-vendoring helpers they sit on. `lib/default.nix` imports each and
surfaces them on `ix.<name>` (`lib/default.nix:96-116`, `164-170`); the
`bunLockFor`/`uvLockFor`/`goUnitFor` factories and the `buildJsSite`/
`buildSvelteSite`/`buildNpmVitest`/`buildUvApplication`/`buildGradleFatJar`/
`buildZigPackage`/`buildLibghosttyVt`/`goUnit` builders ride
`sharedHelpers`/`ixReturn` (`lib/default.nix:398-426`, `494-518`).

The shared shape: dependency hashes come from the project's own lockfile, so
updating deps is `<pkgmgr> lock` + commit, not maintaining a separate Nix hash.
Most builders are curried `pkgs: args:` so they build for the caller's system.

## Lockfile vendoring

- `bunLockFor pkgs` (`lib/build/bun-lock.nix`, `lib/default.nix:96-100`): parse a
  `bun.lock`/`package.json` and build the offline node_modules
  (`buildNodeModules`, `lib/build/bun-lock.nix:137`). The sidecar
  `bun-lock-to-json.js` converts Bun's lock to JSON. Consumed by the site
  builders, not usually called directly.
- `uvLockFor pkgs` (`lib/build/uv-lock.nix`, `lib/default.nix:110-114`): read a
  `uv.lock`, convert uv hashes to SRI, and build a wheelhouse of fetched
  distributions (`buildWheelhouse`) for offline install. Consumed by
  `buildUvApplication`.

## JS / TS sites

- `buildJsSite pkgs { ... }` (`lib/build/js-site.nix`): build a static frontend
  from a locked npm or Bun project; pick the package manager with
  `packageManager` (`lib/build/js-site.nix:6-17`). Dependencies are built
  separately and linked in, so source-only changes do not reinstall.
- `buildSvelteSite pkgs { pname, version, src, packageManager?, distDir?, serve?,
  devServer? }` (`lib/build/svelte-site.nix`): wraps the same package-manager
  branching with a static-preview server (`miniserve` over the immutable output)
  and a checkout dev server (Vite from a mutable checkout). Used for the repo
  site and the dashboard UI (`lib/per-system.nix:427-440`,
  `lib/rust/workspace.nix:39-51`).
- `buildNpmVitest pkgs { pname, version, src, preTest?, ... }`
  (`lib/build/npm-vitest.nix`): run a Vitest browser-mode suite in the sandbox
  with playwright's bundled chromium. Exposes `all` (one runCommand for the whole
  suite, good as a `checks.<name>`) and `cases.<id>` (one per `#test`, gated on a
  single `vitest list` IFD, `lib/build/npm-vitest.nix:1-13`).

## Other language builders

- `buildUvApplication pkgs { srcRoot | src, ... }`
  (`lib/build/uv-application.nix`): build a Python app from a uv project.
  Distributions are fetched into a wheelhouse, installed offline into a venv, the
  local project built as a wheel, and `ty` type-checks the venv by default
  (matching `writePythonApplication`, `lib/build/uv-application.nix:4-16`).
- `goUnit` / `goUnitFor pkgs` (`lib/build/go-unit.nix`, `lib/default.nix:164-170`):
  Go build helpers taking `go = languages.go`; `buildWorkspace`
  (`lib/build/go-unit.nix:241`, exported `278`) builds a Go workspace with
  `buildPackage`/`testPackage` per package, keyed by a `vendorHashKey`.
- `buildGradleFatJar { pname, version, src, verificationMetadata, ... }`
  (`lib/build/gradle-fat-jar.nix`): build a Gradle fat-jar fixed-output and
  network-isolated, with dependency hashes from a Gradle dependency-verification
  XML reproduced into the sandbox and the task run offline
  (`lib/build/gradle-fat-jar.nix:1-18`).
- `buildZigPackage pkgs { pname, version, src, zigDepsHash?, ... }`
  (`lib/build/zig-package.nix`): build a `build.zig`/`build.zig.zon` project;
  remote deps are content-addressed by Zig hashes plus one `zigDepsHash` for the
  realized package cache. Named Zig test steps become separate
  `passthru.tests` derivations so flake checks parallelize them
  (`lib/build/zig-package.nix:3-14`).
- `buildLibghosttyVt pkgs { ghosttySource, version? }`
  (`lib/build/libghostty-vt.nix:10-14`): build ghostty's VT engine as a
  standalone C library via `-Demit-lib-vt=true` (parser, screen model,
  render-state API; no GUI). Emits `libghostty-vt.a`, a self-contained
  `.dylib`/`.so`, the `ghostty/` headers, and a pkg-config file. Linked by
  `ix-vt-sys` in the Rust workspace (`lib/rust/workspace.nix:26-31`).
