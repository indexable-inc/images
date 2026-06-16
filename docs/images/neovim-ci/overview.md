# neovim-ci

`images/dev/neovim-ci` is a Neovim Linux CI image: the toolchain and test
dependencies needed to build and test Neovim upstream inside an ix-backed job.
Flake output `.#neovim-ci`.

## What it builds

`images/dev/neovim-ci/default.nix` (44 lines):

- `ix.image.name = "neovim-ci"` (`:9`).
- builds a Python with the `pynvim` host module:
  `python = pkgs.python3.withPackages (ps: [ ps.pynvim ])` (`:3-6,39`).
- ships the full Neovim build/test dependency set in `environment.systemPackages`
  (`:11-43`): build tools `cmake`, `gcc`, `gnumake`, `ninja`, `pkg-config`,
  `gettext`, `unzip`; the LLVM 21 toolchain (`llvmPackages_21.clang`,
  `clang-tools`, `:41-42`); the Lua stack `luajit`, `lua51Packages.{lpeg,
  luafilesystem,luv}`; language runtimes `nodejs`, `perl`
  (`perlPackages.Appcpanminus`, `NeovimExt`), `ruby`, `zig`; and test/lint helpers
  `attr`, `diffutils`, `fish`, `glibcLocales`, `inotify-tools`, `shellcheck`,
  `stylua`, `ts_query_ls`, `xdg-utils`.

There is no service: this image is a build/test environment, not a long-running
daemon. It composes only the auto-enabled base profile plus this package set.

## Build

```
nix build .#neovim-ci
```

Run the image as a VM and execute Neovim's test suite (the `pynvim` host, the Lua
deps, clang/luajit, and the language providers are all present). The base profile
supplies the operator shell, git, and editors; see [common](../common.md).

## Notes

- No eval test group is attached to this image name; its contract is the package
  list above plus the shared platform invariants.
- The package set tracks Neovim upstream CI dependencies; bump the pins in
  `default.nix` when upstream's CI requirements change.
