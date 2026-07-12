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

  # Global gitignore entries: editor droppings, build outputs, and agent/tool
  # scratch state that should never be committed from any checkout. Lines are
  # ordered as git evaluates them (later lines win), so keep the `!CMakeLists.txt`
  # / `!*.cmake` re-includes after the CMake excludes they carve out of.
  globalIgnores = [
    ".claude/settings.local.json"
    ".codex-output/"
    ".codex-reports/"
    "target-clippy"
    "node_modules"
    "target-check"
    "**/target-clippy"
    "**/_build"
    "dump*.json"
    ".idea"
    "*.xcuserstate"
    "xcuserdata/"
    ".DS_Store"
    ".direnv"
    "CMakeCache.txt"
    "CMakeFiles/"
    "cmake_install.cmake"
    "Makefile"
    "*.cmake"
    "!CMakeLists.txt"
    "!*.cmake"
    "_deps/"
    "cpm-package-lock.cmake"
    "build/"
    ".zsync"
    "buck-out"
    ".wrangler"
    "**/.claude/settings.local.json"
    "**/.claude/worktrees/"
    "result"
    "target"
    ".vercel"
    ".sgrep"
    ".pytest_cache/"
    ".ruff_cache/"
    ".worktrees/"
    ".Codex/"
  ];
}
