# launchk

`packages/launchk` builds the checked source at `index/views/launchk`. The package is macOS-only because it talks to launchd over XPC.

## source ownership

`index/lib/default.nix` exposes the tracked path as `ix.launchkSrc`. `.jj-views.toml` maps that path to the `indexable-inc/ix` branch `views/index/views/launchk`, anchored to `intellekthq/launchk` revision `6f5f09e0`. Package evaluation needs no source URL or source flake.

The view history owns the ix source change. The window title reads `CARGO_PKG_VERSION`, so a Nix build does not need a `.git` directory or the `git-version` crate.

## build

`index/packages/launchk/default.nix` uses `rustPlatform.buildRustPackage` with the view's committed `Cargo.lock`. `rustPlatform.bindgenHook` supplies libclang for `xpc-sys`. Build and test flags select the `launchk` package. The package and flake outputs are limited to Darwin systems.

## update

1. Create a dedicated jj workspace for ix and edit `index/views/launchk` there.
2. Commit the upstream change and any ix source change in the host history.
3. Run the Launchk package build and tests from the checked view.
4. Push the derived view with `jj views push launchk --branch views/index/views/launchk --allow-default-branch`, then push the ix bookmark.
