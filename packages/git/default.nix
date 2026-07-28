{
  autoconf,
  git,
  ix,
  lib,
}: let
  # The indexable-inc/git jj megamerge (git-src input, on the v2.55.0 tag
  # base): linked worktrees borrow the common-dir submodule object store via
  # alternates instead of re-cloning every submodule from the network (#3610).
  patchedSrc = ix.gitSrc;
in
  # The nixpkgs recipe expects git's version to match the source it patches;
  # a nixpkgs git bump with a stale git-src pin would silently build the old
  # tree under the new label, so fail eval until the pin is advanced.
  assert lib.assertMsg (git.version == "2.55.0") ''
    packages/git: nixpkgs git is ${git.version} but git-src pins v2.55.0.
    Repin git-src by jj-rebasing indexable-inc/git onto the matching
    upstream tag.'';
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
