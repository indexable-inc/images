# ix-vt: the public safe wrapper crate.
#
# Selected from the shared Rust workspace graph as a library target. The graph
# (lib/rust-workspace.nix) injects the libghostty-vt link search path and the
# `IX_VT_GHOSTTY_LIB_DIR` build-script env so ix-vt-sys links cleanly; this file
# only picks the library and its tests out of that graph.
{ ix, ... }:

ix.cargoUnit.selectLibraryWithTests ix.rustWorkspace.units {
  library = "ix_vt";
  packageName = "ix-vt";
  meta.description = "Safe Rust wrapper over libghostty-vt";
}
