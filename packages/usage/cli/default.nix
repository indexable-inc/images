{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "ix-usage";
  meta = {
    description = "Agent-first CLI over the local ix usage store: consent, captured failures as JSON, spool compaction, and count-only uploads";
    license = lib.licenses.mit;
    mainProgram = "ix-usage";
  };
}
