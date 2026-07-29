# Shared git defaults: attribute and global-ignore lists that are general
# policy, not per-user preference.
#
# Both values are plain lists of lines; render them with something like
# `pkgs.writeText "gitattributes" (lib.concatLines ix.gitDefaults.astMergeAttributes)`
# and point `~/.config/git/attributes` / `core.excludesfile` at the result.
{
  # One `<glob> merge=ast-merge` line per language the AST merge driver
  # supports, so structural (syntax-aware) merges apply everywhere the driver
  # is configured. Consumers still need the `merge.ast-merge.driver` gitconfig
  # entry; this only routes the file types to it.
  astMergeAttributes = [
    "*.rs merge=ast-merge"
    "*.ts merge=ast-merge"
    "*.tsx merge=ast-merge"
    "*.mts merge=ast-merge"
    "*.cts merge=ast-merge"
    "*.js merge=ast-merge"
    "*.jsx merge=ast-merge"
    "*.mjs merge=ast-merge"
    "*.cjs merge=ast-merge"
    "*.py merge=ast-merge"
    "*.pyi merge=ast-merge"
    "*.go merge=ast-merge"
    "*.json merge=ast-merge"
    "*.jsonc merge=ast-merge"
    "*.toml merge=ast-merge"
    "*.yaml merge=ast-merge"
    "*.yml merge=ast-merge"
  ];

  # Global gitignore entries for `core.excludesFile`: editor droppings, build
  # outputs, and agent/tool scratch that should never be committed from any
  # checkout.
  #
  # A pattern here applies to every repository, including ones whose layout it
  # cannot know, so the anchoring rule below is load-bearing rather than style.
  # An entry with no leading `/` matches at EVERY depth, and git offers no
  # warning when that swallows a real source directory: the file simply does not
  # appear in `git status`, and `git add -A` skips it. `build/` cost 20 minutes
  # that way, hiding nix's `src/libstore/unix/build/*.cc` from a commit, which
  # surfaced later as a meson error pointing nowhere near the cause (ENG-10982).
  # The same shape as the rsync `result*` incident recorded in CLAUDE.md.
  #
  # Verified, not assumed: in `core.excludesFile` a leading `/` anchors to the
  # top of the working tree. With `build/ target result Makefile`, a probe repo
  # hid `src/libstore/unix/build/mach-o.cc`, `deep/target/x.rs`,
  # `deep/result/y.txt` and `src/Makefile`; with `/build/ /target /result
  # /Makefile` all four were visible again.
  #
  # So every entry must be one of three things, and `globalIgnores` below throws
  # at eval time if one is none of them:
  #   - anchored with a leading `/`, or
  #   - a re-include (`!`), which can only un-hide, or
  #   - living under a dot-directory or named as a dotfile, or
  #   - listed in `neverSourceNames`, each with the reason it cannot be source.

  globalIgnores = let
    entries = [
      # Editor and OS droppings.
      ".DS_Store"
      ".idea"
      "*.xcuserstate"
      "xcuserdata/"
      ".direnv"
      ".zsync"
      ".vercel"
      ".sgrep"
      ".pytest_cache/"
      ".ruff_cache/"

      # Agent and tool scratch. These stay unanchored on purpose: worktrees and
      # nested checkouts put a `.claude` at arbitrary depth, and every path here
      # sits under a dot-directory, so none of them can shadow source.
      "**/.claude/settings.local.json"
      "**/.claude/worktrees/"
      "**/.claude/fix"
      ".codex-output/"
      ".codex-reports/"
      # Matches `.codex/` too on darwin, where core.ignorecase is true.
      ".Codex/"
      ".worktrees/"

      # Generated names distinctive enough to stay unanchored: no project writes
      # a source file called any of these, so depth cannot hurt.
      "node_modules"
      "buck-out"
      "target-clippy"
      "target-check"
      "CMakeCache.txt"
      "CMakeFiles/"
      "cmake_install.cmake"
      "_deps/"
      "cpm-package-lock.cmake"

      # Build outputs whose names are also ordinary source directory names.
      # Anchored, because unanchored is exactly the ENG-10982 failure: nix keeps
      # C++ sources in `src/libstore/unix/build/`, and `target` and `result`
      # appear as fixture and module names in plenty of trees.
      "/build/"
      "/_build/"
      "/target"
      "/result"
      "/result-*"
      # Was `dump*.json` at every depth, which also hid checked-in fixtures
      # named e.g. `dump-schema.json`.
      "/dump*.json"

      # Deliberately absent, each removed with a reason:
      #
      # `Makefile` hid every hand-written Makefile in every repo at every
      # depth. It was here for in-source CMake builds; the generated CMake
      # names above cover those without hiding source.
      #
      # `*.cmake` plus `!*.cmake` cancelled out, so neither did anything, and
      # the pair also re-included `cmake_install.cmake` two lines above it,
      # defeating that entry. Dropping both makes the CMake block mean what it
      # says and leaves hand-written `.cmake` modules visible (probed).
      #
      # `!CMakeLists.txt` negated a pattern that never matched it.
      #
      # `**/target-clippy` duplicated `target-clippy`: with no slash, a pattern
      # already matches at every depth.
      #
      # `.claude/settings.local.json` was a subset of the `**/` form below it.
    ];

    # Names that cannot be a source file in any repository, so they are safe
    # unanchored. The value is why, because a bare list here would be the same
    # unexplained fence this whole comment exists to prevent.
    neverSourceNames = {
      node_modules = "npm/pnpm/yarn dependency tree, always generated";
      buck-out = "buck2 output root, name reserved by the tool";
      target-clippy = "our own clippy target dir, see rust-checks";
      target-check = "our own cargo-check target dir";
      "CMakeCache.txt" = "cmake configure cache, generated";
      "CMakeFiles/" = "cmake per-directory scratch, generated";
      "cmake_install.cmake" = "cmake install script, generated";
      "_deps/" = "CPM/FetchContent download root, generated";
      "cpm-package-lock.cmake" = "CPM lock, generated";
      "*.xcuserstate" = "Xcode per-user UI state, binary and generated";
      "xcuserdata/" = "Xcode per-user directory, generated";
    };

    isAnchored = entry: builtins.substring 0 1 entry == "/";
    isReinclude = entry: builtins.substring 0 1 entry == "!";
    # True when any path component starts with a dot, so the entry lives under
    # a dot-directory or names a dotfile.
    hasDotComponent = entry: builtins.match "(.*/)?\\.[^/]*(/.*)?" entry != null;
    isNamedSafe = entry: builtins.hasAttr entry neverSourceNames;

    offenders =
      builtins.filter (
        entry: !(isAnchored entry || isReinclude entry || hasDotComponent entry || isNamedSafe entry)
      )
      entries;
  in
    if offenders == []
    then entries
    else
      throw (
        "lib/util/git-defaults.nix: these globalIgnores entries match at every depth,\n"
        + "so they could hide real source files from `git status` and `git add`:\n\n"
        + builtins.concatStringsSep "" (builtins.map (entry: "  - " + entry + "\n") offenders)
        + "\nPick one: anchor it to the worktree root with a leading \"/\", or add it to\n"
        + "`neverSourceNames` with the reason it can never be a source file. See the\n"
        + "comment above `globalIgnores` for what this defends against (ENG-10982).\n"
      );
}
