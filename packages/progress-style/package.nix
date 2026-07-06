{
  id = "progress-style";
  inRustWorkspace = true;
  # Standalone read-only mirror repo, kept in sync by the mirror-sync
  # workflow. See packages/mirror.
  mirror.repo = "indexable-inc/progress-style";
  passthruTests = true;
}
