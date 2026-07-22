{
  bundledSource,
  importTest,
  lib,
  pkgs,
  testsRoot,
}: let
  # Notion REST client: `import notion`, then `await notion.search(query)` /
  # `page(id)` / `blocks(id)` / `db_query(id)` / `page_create` / `blocks_append`
  # / `page_update`. Pure Python over the already-bundled httpx + polars; reads
  # NOTION_API_KEY from the environment at call time. Cross-platform.
  notionPythonSource = bundledSource {
    name = "ix-mcp-notion-python-source";
    path = ./.;
  };
  notionModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-notion-python-module"
    {
      strictDeps = true;
      meta.description = "Notion REST client bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/notion"
      mkdir -p "$site"
      cp -r ${notionPythonSource}/notion/. "$site/"
    ''
  );
  notionBundled = importTest [notionModule] "notion" "import notion, asyncio; print('notion-ok', all(asyncio.iscoroutinefunction(getattr(notion, n)) for n in ('search', 'page', 'blocks', 'db_query', 'page_create', 'blocks_append', 'page_update')), notion.__version__)";
  notionTestPython = pkgs.python3.withPackages (ps: [
    ps.pytest
    ps.httpx
    ps.polars
    ps.pydantic
    notionModule
  ]);
  notionTestSource = builtins.path {
    name = "ix-mcp-notion-test";
    path = testsRoot + "/test_notion.py";
  };
  typeHintSupport = builtins.path {
    name = "ix-mcp-notion-type-hint-support";
    path = testsRoot + "/type_hint_support.py";
  };
  notionTests =
    pkgs.runCommand "ix-mcp-notion-tests"
    {
      nativeBuildInputs = [notionTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      cp ${notionTestSource} "$TMPDIR/test_notion.py"
      cp ${typeHintSupport} "$TMPDIR/type_hint_support.py"
      ${lib.getExe notionTestPython} -m pytest "$TMPDIR/test_notion.py" -q -p no:cacheprovider >stdout 2>stderr || {
        echo "ix-mcp notion tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
in {
  module = notionModule;
  tests = {
    inherit
      notionBundled
      notionTests
      ;
  };
}
