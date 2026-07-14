{
  id = "unibind-backend-rs";
  inRustWorkspace = true;
  # Library-only (client emission runs through unibind-gen's `rs` target);
  # the packageSet + flake exposure is what lets the per-crate tests build
  # natively on Darwin (`checks.<system>` carries the rust catalog only for
  # x86_64-linux), same shape as ix-vt.
  packageSet = true;
  flake = true;
  overlay = false;
  passthruTests = true;
}
