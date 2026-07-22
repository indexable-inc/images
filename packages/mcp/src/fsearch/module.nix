{
  bundledSource,
  bundledTestPython,
  lib,
  pkgs,
  shModule,
}: let
  # The `fsearch` filesystem-search module: `grep`/`find`/`spotlight`, each
  # backed by a battle-tested CLI (ripgrep / fd / macOS Spotlight) run as a
  # SEPARATE process via the kernel-private `sh._exec` runner, returning polars
  # frames. Pure Python over the sh runner/polars; cross-platform (spotlight is
  # darwin-only and guards itself). Unlike its predecessor `fff` (a ctypes cdylib
  # that walked the tree in-process and could pin the cores for an hour with no
  # way to interrupt short of killing the kernel), a runaway here is
  # process-isolated and bounded by `_exec`'s timeout + process-group kill.
  # `ripgrep`/`fd` are put on the interpreter wrapper's PATH below so the runner
  # resolves them.
  fsearchPythonSource = bundledSource {
    name = "ix-mcp-fsearch-python-source";
    path = ./.;
  };
  fsearchModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-fsearch-python-module"
    {
      strictDeps = true;
      passthru.ixFirstPartyDeps = [shModule];
      meta.description = "rg/fd/Spotlight-backed grep/find/spotlight bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/fsearch"
      mkdir -p "$site"
      cp -r ${fsearchPythonSource}/fsearch/. "$site/"
    ''
  );
  # End-to-end through the bundled `fsearch` module: plant a temp tree (with a
  # .gitignore'd file), then prove `grep` (ripgrep) and `find` (fd) return the
  # planted hits, respect .gitignore by default, and that `spotlight` raises its
  # darwin guard off macOS. Runs real rg/fd (on the check's PATH); pure local FS,
  # no network, so the build sandbox runs it.
  fsearchTestPy = pkgs.writeText "ix-mcp-fsearch-test.py" ''
    # python
    import asyncio
    import os
    import subprocess
    import sys
    import tempfile

    import polars as pl

    import fsearch

    root = tempfile.mkdtemp()
    os.makedirs(os.path.join(root, "src"))
    with open(os.path.join(root, "hello_world.txt"), "w") as fh:
        fh.write("greetings\nfind me on this line\n")
    with open(os.path.join(root, "src", "main.rs"), "w") as fh:
        fh.write('fn main() {\n    println!("find me on this line");\n}\n')
    # A .gitignore'd file must be skipped by default and surfaced with no_ignore.
    # ripgrep only honors .gitignore inside a git repo, so init one.
    with open(os.path.join(root, ".gitignore"), "w") as fh:
        fh.write("ignored.txt\n")
    with open(os.path.join(root, "ignored.txt"), "w") as fh:
        fh.write("find me on this line\n")
    subprocess.run(["git", "init", "-q", root], check=True)


    async def main() -> None:
        g = await fsearch.grep("find me on this line", root)
        assert isinstance(g, pl.DataFrame), type(g)
        assert list(g.columns) == ["path", "line_number", "col", "match", "line", "abs_offset"], g.columns
        files = set(g["path"].to_list())
        assert all(files), "a match row had an empty path (rg bytes field not decoded?)"
        assert any("hello_world" in f for f in files), files
        assert any("main.rs" in f for f in files), files
        assert not any("ignored.txt" in f for f in files), f"gitignore not respected: {files}"

        # no_ignore surfaces the ignored file; fixed treats the alternation literally.
        gi = await fsearch.grep("find me on this line", root, no_ignore=True)
        assert any("ignored.txt" in f for f in gi["path"].to_list()), gi["path"].to_list()
        plain = await fsearch.grep("greetings|fn main", root, fixed=True)
        assert plain.height == 0, "fixed=True must treat the alternation literally"

        f = await fsearch.find(ext="rs", root=root)
        assert isinstance(f, pl.DataFrame), type(f)
        assert list(f.columns) == ["path", "name", "type", "size", "mtime"], f.columns
        assert any(n == "main.rs" for n in f["name"].to_list()), f["name"].to_list()
        assert set(f["type"].to_list()) == {"file"}, f["type"].to_list()

        d = await fsearch.find(kind="dir", root=root)
        assert any(n == "src" for n in d["name"].to_list()), d["name"].to_list()

        # spotlight is darwin-only: it must raise a clear error elsewhere.
        if sys.platform != "darwin":
            try:
                await fsearch.spotlight("anything", root)
            except fsearch.FsearchError as exc:
                assert "macOS" in str(exc), exc
            else:
                raise AssertionError("spotlight should raise off macOS")

        # issue #1754 bug 3: limit= short-circuits and flags the partial scan.
        # Plant a tree with many matches so a small limit truncates it.
        big = tempfile.mkdtemp()
        for i in range(50):
            with open(os.path.join(big, f"f{i}.txt"), "w") as fh:
                fh.write("needle here\n" * 20)  # 20 matches per file, 1000 total
        capped = await fsearch.grep("needle", big, limit=5)
        assert isinstance(capped, pl.DataFrame), type(capped)  # still a usable frame
        assert isinstance(capped, fsearch.PartialFrame), "a capped scan must be a PartialFrame"
        assert capped.truncated is True
        assert capped.height == 5, capped.height
        assert "limit" in capped.reason, capped.reason
        assert "partial" in repr(capped).lower(), "the repr must surface truncation"

        # A full scan under the limit is a plain frame with no truncated flag.
        full = await fsearch.grep("needle", big, limit=100000)
        assert full.height == 1000, full.height
        assert not isinstance(full, fsearch.PartialFrame)
        assert not hasattr(full, "truncated")

        # A timeout returns the matches found before the deadline, not nothing.
        # A tiny timeout over the big tree is very likely to trip; if the machine
        # is fast enough to finish, the assertion below tolerates a complete scan.
        timed = await fsearch.grep("needle", big, limit=10000000, timeout=0.001)
        if isinstance(timed, fsearch.PartialFrame):
            assert timed.truncated is True
            assert "timed out" in timed.reason, timed.reason

        print("fsearch-ok", fsearch.__version__)


    asyncio.run(main())
  '';
  # fsearch's grep/find run real ripgrep/fd, so the check needs them on PATH
  # (the same two added to the interpreter wrapper). The fsearch closure + the
  # planted tree prove the helpers end to end in the Linux sandbox; spotlight
  # only asserts its darwin guard there.
  fsearchTestPython = bundledTestPython [fsearchModule];
  fsearchBundled =
    pkgs.runCommand "ix-mcp-fsearch"
    {
      nativeBuildInputs = [
        fsearchTestPython
        pkgs.ripgrep
        pkgs.fd
        pkgs.git
      ];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe fsearchTestPython} ${fsearchTestPy} >stdout 2>stderr || {
        echo "ix-mcp fsearch test failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -q '^fsearch-ok' stdout || {
        echo "ix-mcp fsearch test did not print its ok marker:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
in {
  module = fsearchModule;
  tests = {
    inherit
      fsearchBundled
      ;
  };
}
