# The repo's ruff lint policy lives in the checked-in /ruff.toml (selector,
# ignores, and the per-rule rationale); this module derives the inline-flag
# form consumed by every Python gate so the policy never drifts across call
# sites:
#   lib/build/uv-application.nix   (all uv apps)
#   lib/util/writers.nix          (writePythonApplication scripts)
#   packages/mcp/default.nix      (bundled-module strict gate)
#   packages/agent/distiller/default.nix
#   packages/sdk/python/build.nix
#   lib/per-system.nix            (the repo-wide `ruff` lint stage, over ALL .py)
#
# Why flags AND a discovered toml: per-package builds run ruff inside sandboxes
# that do not contain the repo root, so an on-disk config would never be found
# there -- the gates take the policy inline (--select/--ignore/--config). A
# direct `ruff check` in a checkout, meanwhile, discovers ruff.toml and applies
# the same policy, so ad-hoc runs cannot diverge from the gates: ruff's default
# E/F selection used to flag patterns the repo deliberately allows (e.g. the
# importorskip-then-import pattern in tests -> E402), reading as "my change
# broke lint" (#2393).
{lib}: let
  policy = builtins.fromTOML (builtins.readFile ../ruff.toml);
  banMessage = policy.lint.flake8-tidy-imports.banned-api."typing.cast".msg;
  banConfig = ''lint.flake8-tidy-imports.banned-api."typing.cast".msg = "${banMessage}"'';
in {
  inherit (policy.lint) select ignore;
  inherit banMessage banConfig;
  # Drop-in replacement for the old bare `--select ANN`:
  #   ruff check ${ruffAnnArgs} <targets>
  ruffAnnArgs = "--target-version ${policy.target-version} --select ${lib.concatStringsSep "," policy.lint.select} --ignore ${lib.concatStringsSep "," policy.lint.ignore} --config ${lib.escapeShellArg banConfig}";
}
