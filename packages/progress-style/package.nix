{
  id = "progress-style";
  inRustWorkspace = true;
  # Standalone read-only mirror repo, kept in sync by the mirror-sync
  # workflow. See packages/mirror.
  mirror = {
    repo = "indexable-inc/progress-style";
    description = "Shared indicatif progress-bar and spinner styling for ix command-line tools, so every CLI renders the same shape.";
    topics = [
      "rust"
      "indicatif"
      "progress-bar"
      "terminal"
      "cli"
      "ix"
    ];
  };
  passthruTests = true;
}
