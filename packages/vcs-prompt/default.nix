{
  ix,
  lib,
  ...
}:
# No PATH wrapper: both backends call the VCS the way any prompt integration
# does, out of the ambient PATH. The segment only ever renders inside an
# interactive shell whose profile already installs `git` and `jujutsu`
# (users/andrewgazelka/profiles/workstation.nix), and baking a second copy of
# each would duplicate a jj that the profile pins anyway, drag jujutsu's
# from-source Darwin build (plus gnupg) into this closure, and risk answering
# with a different jj than the one the user's own commands run.
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "vcs-prompt";
  meta = {
    description = "Starship VCS segment: jj working-copy state inside a jj workspace, git branch and status everywhere else";
    license = lib.licenses.mit;
    mainProgram = "vcs-prompt";
  };
}
