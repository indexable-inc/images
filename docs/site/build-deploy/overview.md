# build

Genre: recipe. How the site is built, previewed, tested, and deployed. The site
is a JavaScript package (`site/package.json` name `ix-site`); the reproducible
build is a Nix derivation produced by the shared `ix.buildSvelteSite` helper, and
deployment is GitHub Pages via `.github/workflows/pages.yml`.

## Toolchain

- SvelteKit 2 (`@sveltejs/kit ^2.61`), Svelte 5 (`^5.55`), Vite 8, TypeScript 6
  (`site/package.json:15-31`).
- `@sveltejs/adapter-static ^3` (`:17`) prerenders to flat files.
- Runtime deps: `@xyflow/svelte` (diagrams), `mdsvex` (`.svx`), `shiki`
  (highlighting) (`site/package.json:36-40`).
- Two lockfiles are committed: `site/package-lock.json` and `site/bun.lock`. The
  Nix build path uses npm (below); `bun.lock` supports local `bun install`.

## npm scripts (`site/package.json:6-14`)

- `dev` -> `vite dev` - local dev server with HMR.
- `build` -> `svelte-kit sync && svelte-check --tsconfig ./tsconfig.json &&
  eslint . && vite build` - typecheck + lint + production build. Output goes to
  `build/` (the adapter `pages`/`assets` dir, `svelte.config.js:15-16`).
- `preview` -> `vite preview`.
- `check` -> `svelte-kit sync && svelte-check`.
- `lint` -> `eslint .` (flat config, `eslint.config.js`; strict typed rules,
  `no-explicit-any: error`).
- `test` -> `svelte-kit sync && vitest run`; `test:list` enumerates cases as JSON.

## Nix package (`ix.buildSvelteSite`)

Defined in `lib/build/svelte-site.nix`, exported as `ix.buildSvelteSite`
(`lib/default.nix:104`). It wraps the same `buildJsSite`-style
package-manager branching (`lib/build/js-site.nix`) and adds a static preview
command and a checkout dev server. The site instance is configured in `lib/per-system.nix:427-440`:

```nix
siteBuild = ix.buildSvelteSite pkgs {
  pname = "ix-site";
  version = "0.1.0";
  src = siteSrc;            # paths.site: the git-filtered site subtree input
  distDir = "build";        # adapter-static output
  serve   = { name = "ix-site";     routePrefix = "/index"; };
  devServer = { name = "ix-site-dev"; checkoutSubdir = "site"; };
};
```

- `packageManager` is not set, so it defaults to `npm` (`svelte-site.nix:37`);
  deps are built from `package-lock.json` via `pkgs.importNpmLock.buildNodeModules`
  (`svelte-site.nix:111`). `node_modules` is linked in, the `build` script runs in
  a pure sandbox (`buildPhase` `:168-173`), and the output is installed to
  `$out/share/ix-site` (`installDir` default `share/${pname}`, `:42,175-180`).
- `src` is `paths.site`, the `flake = false` path input `./site`
  (`flake.nix:61-64`, `flake.nix:215`), so the build's source identity is scoped
  to the `site/` subtree and edits elsewhere do not re-hash it
  (`per-system.nix:422-425`).
- `serve` builds a miniserve wrapper that serves `$out/share/ix-site` at
  `127.0.0.1:8080` with `--route-prefix /index` and SPA fallback
  (`svelte-site.nix:185-230`). This previews the exact `/index` build Pages
  deploys, without a `BASE_PATH` rebuild.
- `devServer` builds a Nushell wrapper that runs `npm run dev` from a mutable
  checkout (`checkoutSubdir = "site"`), auto-installing `node_modules` when
  missing and passing `--host`/`--port` (`svelte-site.nix:247-306`). Dev state
  (installs, caches, HMR) stays outside the Nix store.

`site` (the flake output) overrides the build to also expose
`passthru.preview`/`passthru.static` (`per-system.nix:443-448`).

## Flake outputs

Wired in `lib/per-system.nix:1069-1070`:

- `nix build .#site` - the GitHub Pages tree; assets land under
  `result/share/ix-site/`.
- `nix run .#site` - boots the miniserve preview at
  `http://127.0.0.1:8080/index/`.
- `nix run .#site-dev` - the checkout dev server
  (`site.passthru.devServer`).

## Tests as checks

`siteTests = ix.buildNpmVitest` (`per-system.nix:450-457`) runs the vitest
browser suite (playwright chromium) in the Nix sandbox, after
`@sveltejs/kit sync` (`preTest`). Exposed as flake checks
(`per-system.nix:1013-1016`): `site-test` (the whole suite) and
`site-case-tests` (a link farm of one derivation per `#test`). See the tests
section in [lib](../lib/overview.md).

## Deploy: GitHub Pages (`.github/workflows/pages.yml`)

On push to `main` (or manual dispatch):

1. `Build site`: `nix build .#site -L` (`pages.yml:38`), with Determinate Nix and
   the `indexable-inc` Cachix cache.
2. `Prepare Pages artifact`: copy `result/share/ix-site/.` into `site-dist/` and
   `touch site-dist/.nojekyll` (`pages.yml:41-44`).
3. `Upload Pages artifact` then a `deploy` job runs `actions/deploy-pages`
   (`pages.yml:49-63`).

The build serves under the `/index` base path (`svelte.config.js:24`), matching
project-pages hosting at `https://indexable-inc.github.io/index/` (the canonical
`siteUrl`, `updates.ts:50`). The header links to `https://ix.dev`
(`+layout.svelte:21`) as the project's external/brand URL; the current source
contains no `static/CNAME`, so a custom-domain deploy would set `BASE_PATH=""`
(per the override note in `svelte.config.js:22-23`). See NOTES in the domain
index for this gap.

## Local recipes

```bash
nix build .#site            # produce result/share/ix-site (Pages tree)
nix run   .#site            # preview at http://127.0.0.1:8080/index/
nix run   .#site-dev        # Vite dev server from the site/ checkout
# from site/ with node_modules installed:
npm run build               # typecheck + lint + vite build -> build/
npm test                    # vitest (browser mode, needs chromium)
```
