# The renderer library, selected from the shared workspace graph so its
# snapshot tests ride `passthru.tests`. Client emission is not here: it goes
# through `unibind-gen rs` / the `rust` target of `unibind.lib.build`, the
# one host-file generator.
{ix, ...}:
ix.cargoUnit.selectLibraryWithTests ix.rustWorkspace.units {
  library = "unibind_backend_rs";
  packageName = "unibind-backend-rs";
  meta.description = "Render the unibind IR into a stabby-based Rust ABI: engine export glue and a generated dlopen client crate";
}
