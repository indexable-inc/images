{
  ix,
  lib,
  rustPlatform,
  starship,
}: let
  # The starship view is based on v1.26.0. Its fork commit resolves the repo
  # root through a jj-or-git ancestor scan rather than gix discovery, so
  # `directory.truncate_to_repo` and `custom.require_repo` recognize a
  # non-colocated jj workspace -- a `.jj` with no `.git`, which is jj's default
  # -- as the repository it is. Without it the prompt prints the absolute path
  # of every jj checkout instead of its name.
  patchedSrc = ix.starshipSrc;
in
  # The nixpkgs recipe pins the tag it fetches and the `cargoHash` for that
  # tag's Cargo.lock; a nixpkgs starship bump with a stale view would build the
  # old tree under the new label, so fail eval until the view advances.
  assert lib.assertMsg (starship.version == "1.26.0") ''
    packages/starship: nixpkgs starship is ${starship.version} but the starship
    view is v1.26.0. Update the starship view to the matching upstream tag.'';
    starship.overrideAttrs (old: {
      src = patchedSrc;

      # The view's Cargo.lock is upstream main's, not the v1.26.0 tag's that
      # nixpkgs pinned cargoHash for (the ancestry repair of 2026-08-09
      # restored the real lockfile). Vendor from the view's own lock.
      cargoDeps = rustPlatform.fetchCargoVendor {
        src = patchedSrc;
        name = "starship-1.26.0-vendor";
        hash = "sha256-IO/H75FKU3/2oAJ8AKerGujMDfun8w4fV7gETMxWOt0=";
      };

      meta =
        (old.meta or {})
        // {
          homepage = "https://github.com/indexable-inc/starship";
        };
    })
