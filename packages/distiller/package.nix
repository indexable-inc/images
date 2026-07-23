{
  id = "distiller";
  packageSet = true;
  flake = true;
  overlay.attrName = "ix-distiller";
  passthruTests = {
    prefix = "distiller";
  };
}
