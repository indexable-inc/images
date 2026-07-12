{
  id = "link-meta";
  inRustWorkspace = true;
  # Library crate: the `stdout_lens!` macro that embeds linking metadata in a
  # binary's `.ix.link` (ELF) / `__DATA,__ix_link` (Mach-O) section, plus the
  # matching reader. Consumed by binaries here (see ./demo) and mirrored, in
  # spirit, by the nushell consumer patch in packages/nushell/patches. No
  # standalone artifact, so no flake/packageSet systems.
  passthruTests = true;
}
