# `astlog-scan`: turnkey Nix lint gate for downstream consumers.
#
# Bundles the `astlog` binary with the in-repo `astlog-rules/nix.astlog`
# ruleset so a downstream flake (ix and others) can drop a single command
# into a pre-commit hook or dev shell without re-deriving the rules path or
# threading wrappers through its own per-system layer. Discovers `.nix`
# files in the caller's working directory with `fd` and shells out to
# `astlog scan`; the binary's exit code is the gate.
#
# Index's own `lib/per-system.nix` keeps using `lintStage` for the
# four-stage local lint run (alejandra | statix | deadnix | astlog | astlog-rust);
# this package is the externally consumable surface.
{
  ix,
  writeNushellApplication,
  astlog,
  fd,
  git,
}: let
  rules = ix.paths.root + "/astlog-rules/nix.astlog";
in
  writeNushellApplication {
    name = "astlog-scan";
    runtimeInputs = [
      astlog
      fd
      git
    ];
    meta = {
      description = "Scan a Nix tree with the index-owned astlog Nix lint rules";
      mainProgram = "astlog-scan";
    };
    text = ''
      # nu
      def main [] {
        let repo_root = (^git rev-parse --show-toplevel | str trim)
        cd $repo_root
        let nix_files = (^fd --hidden --extension nix --type file | lines)
        if ($nix_files | is-empty) { return }
        ^astlog scan ${rules} ...$nix_files
      }
    '';
  }
