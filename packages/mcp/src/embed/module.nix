{
  bundledSource,
  importTest,
  pkgs,
}: let
  # `embed`: Python-native code embeddings (chunk / embed / parquet cache /
  # similarity search) for semantic clone detection and code search
  # (index#3417). Pure Python over the bundled numpy + polars; the inference
  # runtime (torch + sentence-transformers on MPS) is darwin-only and gated in
  # `darwinExtraPackages`, imported lazily inside the functions that need it.
  embedPythonSource = bundledSource {
    name = "ix-mcp-embed-python-source";
    path = ./.;
  };
  embedModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-embed-python-module"
    {
      strictDeps = true;
      meta.description = "In-process code-embedding battery (torch/MPS) bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/embed"
      mkdir -p "$site"
      cp -r ${embedPythonSource}/embed/. "$site/"
    ''
  );
  # `embed` imports everywhere (its torch/MPS runtime loads lazily inside the
  # embedding calls), so the import test runs on Linux too.
  embedBundled = importTest [embedModule] "embed" "import embed; print('embed-ok', embed.__version__)";
in {
  module = embedModule;
  tests = {
    inherit
      embedBundled
      ;
  };
}
