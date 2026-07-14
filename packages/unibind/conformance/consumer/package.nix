{
  id = "unibind-conformance-consumer";
  inRustWorkspace = true;
  packageSet = true;
  # Flake-exposed so the conformance and drift gates build natively on
  # Darwin via `nix build .#unibind-conformance-consumer.passthru.tests.<name>`
  # (`checks.<system>` carries the rust catalog only for x86_64-linux).
  flake = true;
  overlay = false;
  # Gate the Rust-ABI runner next to the Python one
  # (`unibind-conformance-run`): `unibind-conformance-rs-integration` and
  # `unibind-conformance-rs-client-drift`.
  passthruTests = {
    prefix = "unibind-conformance-rs";
  };
}
