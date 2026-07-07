# Demo for the `exportHistory` git-fetcher patch (RFC 0010): render a
# Markdown changelog for one subtree of an input, entirely from eval data.
#
# Run with the patched nix (the stock daemon/client lacks the feature):
#
#   nix build .#nix-ix
#   ./result/bin/nix eval --raw \
#     --extra-experimental-features 'git-export-history flakes nix-command' \
#     --impure --expr 'import ./packages/nix/nix/examples/history-changelog.nix {}'
#
# The input is pinned by rev, so the output is byte-for-byte reproducible:
# history is a pure function of the locked revision (like revCount), cached
# in the fetcher cache and never written to lock files.
{
  # A small, stable, public repo pinned by rev.
  url ? "https://github.com/NixOS/patchelf.git",
  rev ? "99c24238981b7b1084313aca8f5c493bb46f302c", # tag 0.18.0
  subtree ? "src/",
  depth ? 60,
}: let
  # The demo exists to exercise the patched fetcher's eval-time surface;
  # a pkgs.* fetcher (a derivation) cannot expose `history` to eval.
  # astlog-ignore: no-builtins-fetch
  src = builtins.fetchGit {
    inherit url rev;
    exportHistory = true;
    historyDepth = depth;
  };

  touches = prefix: commit:
    builtins.any (p: (builtins.substring 0 (builtins.stringLength prefix) p.path) == prefix)
    commit.paths;

  shortRev = commit: builtins.substring 0 7 commit.rev;
  firstLine = s: builtins.head (builtins.split "\n" s);

  renderCommit = commit: "- ${firstLine commit.message} (`${shortRev commit}`, ${commit.author.name})";

  interesting = builtins.filter (touches subtree) src.history;
in ''
  # Changelog: `${subtree}` of ${url}

  ${builtins.toString (builtins.length interesting)} of the ${builtins.toString (builtins.length src.history)} exported commits (historyDepth = ${builtins.toString depth}, counted in generations like `git clone --depth`) touch `${subtree}`; tip `${builtins.substring 0 7 rev}`.

  ${builtins.concatStringsSep "\n" (map renderCommit interesting)}
''
