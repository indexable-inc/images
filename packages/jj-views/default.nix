{
  lib,
  repoPackages,
}:
# The second binary of the jj workspace `packages/jj` already builds. Its
# `cargoTargets` names `-p jj-views` in the second cargo execution so the
# clippy gate can see the crate, and that same execution roots the binary, so
# `binaries` already carries it -- selecting it here adds a rustc unit for the
# bin target and reuses every dependency unit the `jj` build already has.
#
# A separate package rather than a second output of `jj`: this is a repository
# tool for maintaining derived-view subtrees, not part of the VCS anyone
# installs, and nothing that wants `jj` wants it in the same closure.
#
# It exists as a package at all because the vendoring it performs is not
# reproducible without it. `~/.config/nix` carries ix at `ix/` as a derived
# view, and both directions of that -- `jj-views unfilter` to take upstream ix
# commits in, `jj-views derive` to publish work back -- are this binary. Three
# separate agent sessions built it by hand into /tmp before this landed.
repoPackages.jj.passthru.workspace.binaries."jj-views".overrideAttrs (old: {
  meta =
    (old.meta or {})
    // {
      description = "Deterministic path filters over git history, deriving a child view from a parent monorepo";
      homepage = "https://github.com/indexable-inc/jj";
      license = lib.licenses.asl20;
      mainProgram = "jj-views";
      platforms = lib.platforms.unix;
    };
})
