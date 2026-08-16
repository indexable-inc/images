{
  id = "ix-credential";
  packageSet = true;
  flake = true;
  # `programs.git-token-auth` registers `ix-credential token-helper` as git's
  # credential helper on a guest, so a NixOS module has to be able to take it
  # as `pkgs.ix-credential`.
  overlay = true;
  inRustWorkspace = true;
  passthruTests = true;
}
