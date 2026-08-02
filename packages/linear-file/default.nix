# linear-file: file one Linear issue and print its identifier and URL as JSON.
#
# The house rules require filing a ticket the moment friction is hit, and there
# was no tool: every filing was a hand-built GraphQL call with a heredoc and a
# jq payload, about fifteen lines when correct. The rules also say a tool of
# ours lacking structured output gets its interface fixed rather than worked
# around, so this is that fix.
{
  ix,
  lib,
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "linear-file";
  meta = {
    description = "File a Linear issue from the command line and print the identifier and URL as JSON";
    license = lib.licenses.mit;
    mainProgram = "linear-file";
  };
}
