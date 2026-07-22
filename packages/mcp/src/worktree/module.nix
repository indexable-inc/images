{
  bundledSource,
  bundledTestPython,
  lib,
  nixModule,
  pkgs,
  shModule,
}: let
  # Git worktrees as the unit of isolated work: `import worktree`, then
  # `wt = await worktree.add("my-fix")` checks out a new branch in its own tree,
  # `await wt.build(".#mcp")` stages + nix-builds it, `worktree.list()` is a
  # DataFrame. Pure Python over the bundled sh/nix/polars; cross-platform.
  worktreePythonSource = bundledSource {
    name = "ix-mcp-worktree-python-source";
    path = ./.;
  };
  worktreeModule = pkgs.python3.pkgs.toPythonModule (
    pkgs.runCommand "ix-mcp-worktree-python-module"
    {
      strictDeps = true;
      passthru.ixFirstPartyDeps = [
        nixModule
        shModule
      ];
      meta.description = "Git-worktree helper bundled into the ix-mcp interpreter";
    }
    ''
      site="$out/${pkgs.python3.sitePackages}/worktree"
      mkdir -p "$site"
      cp -r ${worktreePythonSource}/worktree/. "$site/"
    ''
  );
  # The worktree module: drive real `git worktree` against a throwaway repo
  # (git is on PATH in this sandbox). Proves add() creates a new branch in its
  # own tree (and checks out an existing branch instead of recreating it),
  # list() is a DataFrame marking the current tree, the Worktree is os.PathLike
  # and `wt / "x"` joins onto it, commit() stages new files, and remove() drops
  # the tree. Pure git + the bundled sh, so the sandbox runs it.
  worktreeTestPy = pkgs.writeText "ix-mcp-worktree-test.py" ''
    # python
    import asyncio
    import os
    import pathlib
    import subprocess
    import tempfile

    import polars as pl

    import worktree


    def _git(*args, cwd):
        subprocess.run(["git", "-C", cwd, *args], check=True, capture_output=True)


    async def main():
        repo = tempfile.mkdtemp()
        _git("init", "-q", cwd=repo)
        _git("config", "user.email", "t@t", cwd=repo)
        _git("config", "user.name", "t", cwd=repo)
        _git("commit", "--allow-empty", "-q", "-m", "init", cwd=repo)

        # add() creates a NEW branch in its own tree off HEAD.
        wt = await worktree.add("feature-x", repo=repo)
        assert wt.branch == "feature-x", wt
        assert wt.path.is_dir(), wt.path

        # os.PathLike + `wt / "x"` join onto the tree.
        assert os.fspath(wt) == str(wt.path), wt
        (wt / "hello.txt").write_text("hi")

        # list() is a DataFrame; exactly one tree is `current` (the main one), and
        # the new worktree is not it.
        lst = worktree.list(repo)
        assert isinstance(lst, pl.DataFrame) and "current" in lst.columns, lst.columns
        assert "feature-x" in set(lst["branch"].to_list()), lst
        assert lst.filter(pl.col("current")).height == 1, lst
        assert not lst.filter(pl.col("branch") == "feature-x")["current"][0], lst

        # commit() stages the new (untracked) file, so it lands in the commit.
        c = await wt.commit("add hello")
        assert c.ok, c.text
        tracked = subprocess.run(
            ["git", "-C", str(wt.path), "ls-files"], capture_output=True, text=True
        ).stdout
        assert "hello.txt" in tracked, tracked

        # An existing branch is checked out (not recreated) by add().
        _git("branch", "existing", cwd=repo)
        wt2 = await worktree.add("existing", repo=repo)
        assert wt2.branch == "existing", wt2

        # remove() drops the tree (force discards uncommitted changes in it);
        # main + feature-x remain.
        rm = await wt2.remove(force=True)
        assert rm.ok, rm.text
        assert worktree.list(repo).height == 2, worktree.list(repo)

        print("worktree-ok")


    asyncio.run(main())
  '';
  worktreeTestPython = bundledTestPython [worktreeModule];
  worktreeSmoke =
    pkgs.runCommand "ix-mcp-worktree-smoke"
    {
      nativeBuildInputs = [
        worktreeTestPython
        pkgs.git
      ];
      strictDeps = true;
    }
    ''
      export HOME=$TMPDIR/home
      mkdir -p "$HOME"
      ${lib.getExe worktreeTestPython} ${worktreeTestPy} >stdout 2>stderr || {
        echo "ix-mcp worktree smoke failed:" >&2
        cat stdout stderr >&2
        exit 1
      }
      grep -qx 'worktree-ok' stdout || {
        echo "ix-mcp worktree smoke did not confirm the worktree module:" >&2
        cat stdout stderr >&2
        exit 1
      }
      mkdir -p "$out"
    '';
in {
  module = worktreeModule;
  tests = {
    inherit
      worktreeSmoke
      ;
  };
}
