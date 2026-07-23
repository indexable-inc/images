{
  id = "indexer";
  packageSet = true;
  flake = true;
  overlay.attrName = "ix-indexer";
  inRustWorkspace = true;
  passthruTests = true;
}
