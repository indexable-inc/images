{
  bundledSource,
  importTest,
  lib,
  mcpPython,
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
  # The battery's CLI (index#3905): `python -m embed` on the exact interpreter
  # the kernels run (`mcpPython`, not a per-module test env), so the torch/MPS
  # runtime and the per-model-revision parquet cache behave identically to the
  # in-kernel `import embed` path. Surfaced as `nix run .#embed` through
  # lib/per-system.nix via the package's `passthru.embedCli`; Elixir callers
  # shell out to it (`embed dupes <root> --k 40 --json`).
  embedCli =
    pkgs.runCommand "ix-mcp-embed-cli"
    {
      nativeBuildInputs = [pkgs.makeWrapper];
      strictDeps = true;
      meta = {
        description = "Embedding duplicate-code finder and semantic code search (the bundled `embed` module as a CLI)";
        mainProgram = "embed";
      };
    }
    ''
      mkdir -p $out/bin
      makeWrapper ${lib.getExe mcpPython} $out/bin/embed \
        --add-flags "-m embed"
    '';
  # The CLI boundary, provable offline: `--help` exits 0, and a missing root
  # fails loudly (EmbedError on stderr, nonzero exit) before any model or
  # cache is touched. Runs against the shipped wrapper itself, so it rides
  # `mcpPython`'s closure by construction -- the artifact under test is the
  # whole-interpreter wrapper, not this module's test env.
  embedCliSmoke =
    pkgs.runCommand "ix-mcp-embed-cli-smoke"
    {
      nativeBuildInputs = [embedCli];
      strictDeps = true;
    }
    ''
      embed --help >/dev/null
      if embed dupes /does-not-exist --json 2>stderr; then
        echo "embed dupes on a missing root should exit nonzero" >&2
        exit 1
      fi
      grep -q "is not a directory" stderr || {
        echo "expected the EmbedError message on stderr, got:" >&2
        cat stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
in {
  module = embedModule;
  cli = embedCli;
  tests = {
    inherit
      embedBundled
      embedCliSmoke
      ;
  };
}
