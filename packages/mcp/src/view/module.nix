{
  bundledSource,
  bundledTestPython,
  ixNotebookMcpModule,
  lib,
  pkgs,
}: let
  # Pretty, composable views of files and search results (view.ls/tree/grep/find
  # return polars DataFrames; view.cat/json/diff return highlighted Code). Pure
  # Python over the bundled polars/pygments; cross-platform, so every session
  # can `import view`.
  viewPythonSource = bundledSource {
    name = "ix-mcp-view-python-source";
    path = ./.;
  };
  viewModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-view-python-module"
    {
      strictDeps = true;
      passthru.ixFirstPartyDeps = [ixNotebookMcpModule];
      meta.description = "Pretty composable file/search views bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/view"
      mkdir -p "$site"
      cp -r ${viewPythonSource}/view/. "$site/"
    ''
  );
  # The view module: tabular helpers return plain polars DataFrames (so they stay
  # composable), the file helpers return a Code view whose repr is the raw text,
  # and df_html renders the styled table the kernel installs globally. Pure local
  # FS over the bundled view/polars/pygments, so the sandbox runs it.
  viewTestPy = pkgs.writeText "ix-mcp-view-test.py" ''
    # python
    import os
    import tempfile

    import polars as pl

    import view

    # A planted fixture tree, not an interpolation of the whole package dir,
    # which made every file edit in packages/mcp re-run this smoke (#3897).
    base = tempfile.mkdtemp()
    os.makedirs(os.path.join(base, "sub"))
    with open(os.path.join(base, "default.nix"), "w") as fh:
        fh.write("{\n  # fixture\n  x = 1;\n}\n")
    with open(os.path.join(base, "sub", "notes.txt"), "w") as fh:
        fh.write("hello fixture\n")

    lsdf = view.ls(base)
    assert isinstance(lsdf, pl.DataFrame) and "kind" in lsdf.columns, lsdf.columns
    # ls flags git-ignored entries in an `ignored` Boolean column rather than
    # dropping them; outside a git work tree (this temp dir) nothing is ignored.
    assert lsdf.schema["ignored"] == pl.Boolean, lsdf.schema
    assert not lsdf["ignored"].any(), lsdf
    # A DataFrame stays a DataFrame through polars ops (composable).
    assert isinstance(lsdf.filter(pl.col("kind") == "dir"), pl.DataFrame)
    # Content/file search is no longer in `view`: it lives in the top-level
    # `grep`/`find` builtins (rg/fd-backed), exercised by the fsearch check.

    tr = view.tree(base, depth=1)
    assert isinstance(tr, pl.DataFrame) and "depth" in tr.columns

    out = view.df_html(lsdf)
    assert "<table" in out and "rows" in out and "tabular-nums" in out, out[:120]
    # The modern grid ships a client-side filter box, a sortable (clickable,
    # aria-sort) sticky header, and dtype-classed cells -- inline JS/CSS, no CDN.
    assert 'input class="q"' in out and "aria-sort" in out and "sticky" in out, out[:200]
    # Coloring lives in the ONE shared stylesheet keyed by dtype class, not a
    # per-cell style= attribute -- that keeps a wide frame's body small enough
    # for the dashboard's Loro pane diff. A 40x40 int frame must stay well under
    # the ~200KB range that wedged the aggregator, and far below the old build
    # (which repeated a full inline style on every cell).
    wide = pl.DataFrame({f"c{j}": range(40) for j in range(40)})
    wout = view.df_html(wide)
    assert 'style="color:' not in wout, "cells must be class-styled, not inline"
    assert len(wout) < 130000, len(wout)

    # Nested List(Struct)/Struct cells render as boxed sub-tables, not a
    # truncated str(value): the inner field values must reach the HTML.
    nested = pl.DataFrame({"host": ["h1"]}).with_columns(
        mounts=pl.lit([{"mount": "/data", "pct": 91}], dtype=pl.List(pl.Struct({"mount": pl.String, "pct": pl.Int64})))
    )
    nout = view.df_html(nested)
    assert "/data" in nout and ">91<" in nout, nout[:200]
    # A nested cell is a real sub-table (outer + inner), not a truncated repr.
    assert nout.count("<table") >= 2 and "[{" not in nout, nout[:200]

    # A struct field name is attacker-controllable (any frame built from
    # untrusted data); it must be HTML-escaped both in the column-header dtype
    # string and in the nested sub-table, never injected as live markup.
    evil = pl.DataFrame({"x": [1]}).with_columns(
        rec=pl.lit({"<img src=x>": 1}, dtype=pl.Struct({"<img src=x>": pl.Int64}))
    )
    eout = view.df_html(evil)
    # The `not in` clause is the S1 regression guard: it fails on the unfixed
    # header that interpolated the dtype string raw.
    assert "<img src=x>" not in eout and "&lt;img src=x&gt;" in eout, eout[:300]

    c = view.cat(base + "/default.nix", lines=(1, 3))
    assert isinstance(c, view.Code)
    assert repr(c).count("\n") <= 3
    assert "span" in c._repr_html_().lower()

    j = view.json({"a": [1, 2], "b": None})
    assert '"a"' in repr(j) and "span" in j._repr_html_().lower()

    d = view.diff("x\ny\n", "x\nz\n")
    assert "-y" in repr(d) and "+z" in repr(d)

    # edit() applies a replacement and returns it as a highlighted diff.
    import pathlib as _pl_path
    import tempfile as _tmp
    ep = _pl_path.Path(_tmp.mkdtemp()) / "f.txt"
    ep.write_text("alpha\nbeta\n")
    ed = view.edit(ep, "beta", "gamma")
    assert isinstance(ed, view.Code) and "-beta" in repr(ed) and "+gamma" in repr(ed), repr(ed)
    assert ep.read_text() == "alpha\ngamma\n", ep.read_text()
    try:
        view.edit(ep, "missing-zzz", "q")
    except ValueError:
        pass
    else:
        raise SystemExit("edit should raise on a missing pattern")
    prev = view.edit(ep, "gamma", "delta", dry_run=True)
    assert "+delta" in repr(prev) and ep.read_text() == "alpha\ngamma\n", "dry_run must not write"

    print("view-ok")
  '';
  viewTestPython = bundledTestPython [viewModule];
  viewSmoke =
    pkgs.runCommand "ix-mcp-view-smoke"
    {
      nativeBuildInputs = [viewTestPython];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe viewTestPython} ${viewTestPy} >stdout 2>stderr || {
        echo "ix-mcp view smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'view-ok' stdout || {
        echo "ix-mcp view smoke did not confirm the view module:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
in {
  module = viewModule;
  tests = {
    inherit
      viewSmoke
      ;
  };
}
