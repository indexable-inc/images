# Python SDK Nix packaging (`ix-sdk-python`)

`packages/ix-sdk-python` is the Nix package that makes the precompiled Python
SDK bindings available in-repo as `pkgs.ix-sdk-python` /
`nix build .#ix-sdk-python`. It does not build the SDK: it fetches the prebuilt
`ix_sdk` wheel from the public R2 bucket by SRI hash and wraps it as a normal
Python module, the index side of the index <-> ix artifact boundary (ENG-2151,
`default.nix:10-14`). The wheel's native `_ix_sdk` cdylib is built, stripped, and
scanned store-clean by ix's `nix/packages/workspace-sdks.nix`, then uploaded to
R2 with `wrangler`; this consumer only downloads and installs it.

See python for the `ix_sdk` API the wheel exposes, and
common for the cross-SDK prebuilt-artifact model.

## Flake metadata (`package.nix`)

```
id = "ix-sdk-python"; packageSet = true; flake = true;
passthruTests.prefix = "ix-sdk-python";
```

`packageSet = true` puts it in `pkgs` (`pkgs.ix-sdk-python`); `flake = true`
exposes it as a flake package output (`nix build .#ix-sdk-python`); the
registry (`packages/registry.nix`) discovers it from this `package.nix`. Tests
register under the `ix-sdk-python` prefix (`package.nix:1-8`).

## Inputs and the wheel catalog

`default.nix` takes `lib`, `pkgs`, and `python3 ? pkgs.python3`; it matches a
consumer's interpreter and the wheel is `cp313-abi3`, so 3.13+ is required
(`default.nix:1-7`). The per-system `catalog` maps `system` to a `{ url; hash; }`
of the published wheel (`default.nix:24-33`):

- `x86_64-linux`: `ix_sdk-0.1.0-cp313-abi3-manylinux_2_34_x86_64.whl`
  (`default.nix:25-28`).
- `aarch64-darwin`: `ix_sdk-0.1.0-cp313-abi3-macosx_11_0_arm64.whl`; its one
  nix-store dylib (libiconv) is repointed at `/usr/lib` so it loads off-nix
  (`default.nix:20-23`, `29-32`).

URLs and SRI hashes live here next to the consumer (not in `flake.lock`), so a
routine bump is: re-publish the wheel to R2 and edit this catalog; each URL path
embeds the wheel's nix-store hash so distinct builds never collide
(`default.nix:16-19`). An eval-time assert rejects a non-`https://` URL or a
non-`sha256-` hash (`default.nix:39-46`). Unsupported systems return an
eval-safe placeholder derivation that fails loudly at realization with
instructions, rather than guessing a wheel (`default.nix:48-60`).

## How it builds

For a supported system (`default.nix:61-124`):

1. `pkgs.fetchurl { inherit (entry) url hash; }` downloads the wheel
   (`default.nix:63`).
2. `python3.pkgs.toPythonModule (...)` wraps a `runCommand` that unzips the
   wheel (`python3 -m zipfile -e`) straight into `$out/${sitePackages}` so
   consumers `import ix_sdk` with no shim (`default.nix:69-90`). `toPythonModule`
   stamps `pythonModule = python3` so the package composes via
   `python3.withPackages`; without it nixpkgs' `hasPythonModule` filter silently
   drops it (the convention from `packages/mcp`, `default.nix:65-68`).
   `passthru` carries `python3`, `wheel`, and `sitePackages`; `meta.platforms`
   is the catalog's systems (`default.nix:74-82`).
3. The result is `overrideAttrs`'d to attach `passthru.tests.import`
   (`default.nix:118-124`).

## Import / surface test

`importTest` builds a real `python3.withPackages` environment and runs
`assertSurface` through it, so the `toPythonModule` wiring cannot silently
regress (`default.nix:104-116`). `assertSurface` (`default.nix:94-102`) imports
`ix_sdk`, checks `__version__`, and asserts the attributes `ix_sdk` depends on
downstream:

- module-level `Client`, `Group`, `GroupMember`;
- `Client` methods `create_group`, `add_group_member`, `create`, `branches`.

Note: `Group`, `GroupMember`, `create_group`, and `add_group_member` are NOT
present in the source-available `sdk/python/ix_sdk/__init__.py` in this tree
(verified absent). They exist in the prebuilt wheel, which is compiled from a
newer/fuller private `crates/ix/sdk-py`. The packaging test therefore validates
the wheel's surface, which is a superset of the public source mirror. See the
domain NOTES.

## Consumers

`packages/ix-fleet` is the in-repo consumer: it calls `pkgs.callPackage
../ix-sdk-python { }` and copies the unpacked `ix_sdk` into its uv-built venv
site-packages, since the SDK is a prebuilt wheel rather than a uv/PyPI
dependency (`packages/ix-fleet/default.nix:7-11`, `60-66`).
