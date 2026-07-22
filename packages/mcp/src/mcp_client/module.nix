{
  bundledSource,
  ixNotebookMcpModule,
  pkgs,
}: let
  # `mcp_client`: connect to any Model Context Protocol server and call its tools
  # from the kernel. Pure Python over the already-bundled `mcp` SDK (no cdylib),
  # so it wraps the SDK's awkward `async with` transport/session context managers
  # in a persistent `Server` driven by one background task. `import mcp_client`,
  # then `await mcp_client.connect(url_or_command)`.
  mcpClientPythonSource = bundledSource {
    name = "ix-mcp-mcp-client-python-source";
    path = ./.;
  };
  mcpClientModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-mcp-client-python-module"
    {
      strictDeps = true;
      passthru.ixFirstPartyDeps = [ixNotebookMcpModule];
      meta.description = "MCP client helper bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/mcp_client"
      mkdir -p "$site"
      cp -r ${mcpClientPythonSource}/mcp_client/. "$site/"
    ''
  );
in {
  module = mcpClientModule;
}
