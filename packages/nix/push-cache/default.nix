# `nix run .#push-cache -- <installable|/nix/store/path>...`: archive store
# closures into a durable local file:// binary cache directory
# (`$IX_PUSH_CACHE_DIR`, default `~/.cache/ix-push-cache`).
#
# Why a local cache and not cache.ix.dev: nothing aarch64-linux is ever
# published there. cache-push.yml realises `cachePushRoots.x86_64-linux` on the
# self-hosted CI host and pushes through that host's loopback attic shim: a
# ghostunnel mTLS tunnel authenticated by the node's cas-fabric leaf cert, plus
# a push JWT delivered by the fleet secret store (atticd signs narinfos
# server-side, so there is no exportable signing key; the push path is fleet
# surface, not a copyable credential). A developer Mac has neither, so an
# aarch64 build that took hours (guest kernel, mesa fork, toolchains for
# `packages.aarch64-linux.panes-guest-image`) evaporates on the next store GC
# and rebuilds from source. This tool keeps those closures in a plain binary
# cache directory outside any store, which the machine's aarch64 builder VM
# (and optionally the host) lists as a `file://` substituter. The durable fix
# is a native aarch64 CI builder pushing to ix-public like x86_64 does.
#
# The cache is unsigned (nix copy to file:// writes no narinfo signatures), so
# a consumer must either sit inside the producing machine's trust domain (the
# builder VM sets `require-sigs = false`; its disks are host-owned anyway) or
# sign the paths separately before trusting them elsewhere.
#
# No pinned nix in the closure: the client must speak the host daemon's
# protocol/experimental-feature set (ca-derivations on hydra), so it uses the
# ambient nix that just ran this app, same as whence and the updaters.
{ix, ...}:
ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
  binary = "push-cache";
  meta = {
    description = "Archive store closures into a local file:// binary cache";
    mainProgram = "push-cache";
  };
}
