{
  bundledSource,
  distillerModule,
  fsearchModule,
  pkgs,
}: let
  # Local Claude Code history search (issue #2245): `await
  # claude_history.search(pattern)` returns one polars row per matching session
  # under ~/.claude/projects -- session id, un-munged cwd, start/end
  # timestamps, hit count, first real user message -- ranked by hit count. Pure
  # Python: ripgrep matching rides the bundled `fsearch`, transcript parsing
  # reuses the distiller's reader (below), so the transcript schema stays owned
  # in one place on the Python side.
  claudeHistoryPythonSource = bundledSource {
    name = "ix-mcp-claude-history-python-source";
    path = ./.;
  };
  claudeHistoryModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-claude-history-python-module"
    {
      strictDeps = true;
      passthru.ixFirstPartyDeps = [
        distillerModule
        fsearchModule
      ];
      meta.description = "Local Claude Code history search bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/claude_history"
      mkdir -p "$site"
      cp -r ${claudeHistoryPythonSource}/claude_history/. "$site/"
    ''
  );
in {
  module = claudeHistoryModule;
}
