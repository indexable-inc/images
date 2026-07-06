{
  id = "build-version";
  inRustWorkspace = true;
  mirror = {
    repo = "indexable-inc/build-version";
    description = "Format a binary's --version line from Nix-stamped build metadata: revision, commit date, and how long ago it was built.";
    topics = [
      "rust"
      "nix"
      "reproducible-builds"
      "versioning"
      "ix"
    ];
  };
  passthruTests = true;
}
