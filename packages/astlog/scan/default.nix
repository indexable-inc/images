# `astlog-scan`: turnkey Nix lint gate for downstream consumers.
#
# Bundles the `astlog` binary with the in-repo `astlog-rules/nix.astlog`
# ruleset so a downstream flake (ix and others) can drop a single command
# into a pre-commit hook or dev shell without re-deriving the rules path or
# threading wrappers through its own per-system layer. Discovers `.nix`
# files in the caller's working directory with `fd` and shells out to
# `astlog scan`; the binary's exit code is the gate.
#
# The working directory and NOT `git rev-parse --show-toplevel`, which this
# used to `cd` to. Since index/ became a subdirectory of ix (ix#9282) the
# enclosing git toplevel is ix's root, so running this from inside index/
# scanned all of ix with index's ruleset and reported findings in files the
# caller never asked about. Both real callers already run from the root of the
# tree they mean to scan -- ix's lint derivation git-inits a source copy and
# cds into it, index's lintStage runs from its own root -- so honouring the
# documented contract is also what they were relying on.
#
# Index's own `lib/per-system.nix` keeps using `lintStage` for the
# four-stage local lint run (alejandra | statix | deadnix | astlog | astlog-rust);
# this package is the externally consumable surface.
{
  ix,
  writeNushellApplication,
  astlog,
  fd,
}: let
  rules = ix.paths.root + "/astlog-rules/nix.astlog";
in
  writeNushellApplication {
    name = "astlog-scan";
    runtimeInputs = [
      astlog
      fd
    ];
    meta = {
      description = "Scan a Nix tree with the index-owned astlog Nix lint rules";
      mainProgram = "astlog-scan";
    };
    text = ''
      # nu
      def main [] {
        let nix_files = (^fd --hidden --extension nix --type file | lines)
        if ($nix_files | is-empty) { return }
        ^astlog scan ${rules} ...$nix_files
      }
    '';
  }
