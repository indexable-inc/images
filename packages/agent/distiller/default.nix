# ix-distiller: distill ReasoningBank-style lessons from local Claude Code
# transcripts into per-(user, project) facts markdown and a `distilled_facts`
# corpus parquet slice that rides the existing archive -> lake -> Mixedbread
# funnel with zero Rust changes (the leader fold has no source allowlist; see
# the parquet contract in packages/search/sink/parquet and ix docs/history-archive.md).
#
# Packaging follows packages/mcp: pure-Python source copied into a pinned
# interpreter via toPythonModule, a makeWrapper entrypoint, and sandbox-run
# passthru tests (pytest over the contract/merge/transcript units plus an
# import smoke test).
{
  ix,
  lib,
  pkgs,
}: let
  distillerSource = builtins.path {
    name = "ix-distiller-python-source";
    path = ./src;
  };
  distillerModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-distiller-python-module"
    {
      strictDeps = true;
      meta.description = "Claude Code transcript distiller package";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/distiller"
      mkdir -p "$site"
      cp -r ${distillerSource}/distiller/. "$site/"
    ''
  );

  # polars: validation re-reader (a second parquet implementation so pyarrow
  # cannot self-certify the slice). pyarrow: the writer -- its embedded Arrow
  # schema says Utf8, which is what the Rust source-parquet reader downcasts
  # to. boto3: the optional --upload leg into the MinIO archive bucket.
  # pydantic: the persisted record models (Item/SessionRecord/State/Row/Verdict)
  # that validate the on-disk state JSON and the corpus rows.
  pythonDeps = ps: [
    ps.polars
    ps.pyarrow
    ps.boto3
    ps.pydantic
  ];
  distillerPython = pkgs.python3.withPackages (ps: pythonDeps ps ++ [distillerModule]);

  # Strict type-checking gate (ENG-3131), mirroring lib/build/uv-application.nix's
  # zuban branch: `zuban check --strict` for correctness plus `ruff check
  # --select ANN` for explicit annotations. The checker resolves third-party
  # imports through a deps-only interpreter (no `distillerModule`, so `distiller`
  # is type-checked from source as a first-party package, not from the installed
  # site-packages copy). mypy.ini scopes two dependency stub gaps (pyarrow
  # re-exports / untyped writer, boto3's untyped `client`); see that file.
  typeCheckPython = pkgs.python3.withPackages pythonDeps;
  mypyConfig = builtins.path {
    name = "ix-distiller-mypy-ini";
    path = ./mypy.ini;
  };
  typeCheck =
    pkgs.runCommand "ix-distiller-typecheck"
    {
      nativeBuildInputs = [
        pkgs.zuban
        pkgs.ruff
      ];
      strictDeps = true;
    }
    ''
      # zuban discovers mypy.ini from the working directory, so assemble a
      # tree holding the config beside the importable `distiller` package.
      cp -r ${distillerSource}/. ./
      cp ${mypyConfig} ./mypy.ini
      zuban check --strict \
        --python-executable ${lib.getExe typeCheckPython} \
        --python-version ${pkgs.python3.pythonVersion} \
        --platform linux \
        distiller
      ruff check --no-cache ${ix.ruffAnnArgs} distiller
      mkdir -p "$out"
    '';

  package =
    pkgs.runCommand "ix-distiller"
    {
      nativeBuildInputs = [pkgs.makeWrapper];
      strictDeps = true;
      meta = {
        description = "Distill Claude Code transcripts into searchable facts (distilled_facts corpus slices)";
        mainProgram = "ix-distiller";
      };
    }
    ''
      mkdir -p $out/bin
      makeWrapper ${lib.getExe distillerPython} $out/bin/ix-distiller \
        --add-flags "-m distiller"
    '';

  testPython = pkgs.python3.withPackages (
    ps:
      pythonDeps ps
      ++ [
        distillerModule
        ps.pytest
      ]
  );
  testsSource = builtins.path {
    name = "ix-distiller-tests";
    path = ./tests;
  };
  unitTests =
    pkgs.runCommand "ix-distiller-pytest"
    {
      nativeBuildInputs = [testPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR
      ${lib.getExe testPython} -m pytest ${testsSource} -q >stdout 2>stderr || {
        echo "ix-distiller unit tests failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      cat stdout
      mkdir -p "$out"
    '';
  importTest =
    pkgs.runCommand "ix-distiller-import"
    {
      nativeBuildInputs = [distillerPython];
      strictDeps = true;
    }
    ''
      ${lib.getExe distillerPython} -c '
      import distiller, distiller.cli, distiller.corpus, distiller.distill
      import distiller.markdown, distiller.state, distiller.transcripts, distiller.upload
      parser = distiller.cli.build_parser()
      args = parser.parse_args(["--days", "3", "--user", "u", "--host", "h"])
      assert args.days == 3.0 and args.bucket == "ix-history"
      print("distiller-ok", distiller.__version__)
      ' >stdout 2>stderr || {
        echo "ix-distiller import test failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -q '^distiller-ok' stdout
      mkdir -p "$out"
    '';
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        python = distillerPython;
        tests =
          (old.passthru.tests or {})
          // {
            pytest = unitTests;
            import = importTest;
            typecheck = typeCheck;
          };
      };
  })
