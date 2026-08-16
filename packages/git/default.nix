{
  autoconf,
  git,
  ix,
  lib,
}: let
  # The Git view is based on v2.55.0. Its fork commit lets linked worktrees use
  # the common submodule object store instead of cloning it again (#3610).
  patchedSrc = ix.gitSrc;
in
  # The nixpkgs recipe expects git's version to match the source it patches;
  # a nixpkgs Git bump with a stale view would build the old tree under the new
  # label, so fail eval until the view advances.
  assert lib.assertMsg (git.version == "2.55.0") ''
    packages/git: nixpkgs git is ${git.version} but the Git view is v2.55.0.
    Update the Git view to the matching upstream tag.'';
    git.overrideAttrs (old: {
      src = patchedSrc;

      # nixpkgs builds git from the kernel.org dist tarball, which ships a
      # generated `configure`; the git tree does not, so generate it before
      # the stdenv configure phase. Versioning still resolves: the tagged
      # tree's GIT-VERSION-GEN carries DEF_VER=v2.55.0.
      nativeBuildInputs = old.nativeBuildInputs ++ [autoconf];
      preConfigure =
        ''
          make configure
        ''
        + (old.preConfigure or "");
    })
