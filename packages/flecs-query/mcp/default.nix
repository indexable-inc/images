{
  ix,
  lib,
  ...
}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "ix-flecs-query-mcp";
  packageName = "flecs-query-mcp";
  meta = {
    description = "Stdio MCP server for parsing and validating Flecs Query Language expressions";
    license = lib.licenses.mit;
    mainProgram = "ix-flecs-query-mcp";
  };
}
