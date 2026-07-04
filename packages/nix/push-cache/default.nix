# `nix run .#push-cache -- <installable>...`: archive the full build closure of
# one or more installables into a durable local file:// binary cache directory
# (`$IX_PUSH_CACHE_DIR`, default `~/.cache/ix-push-cache`).
#
# Why a local cache and not cache.ix.dev: nothing aarch64-linux is ever
# published there. cache-push.yml realises `cachePushRoots.x86_64-linux` on the
# self-hosted CI host and pushes through that host's loopback attic shim — a
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
{writeNushellApplication}:
writeNushellApplication {
  name = "push-cache";
  meta = {
    description = "Archive an installable's full build closure into a local file:// binary cache";
    mainProgram = "push-cache";
  };
  # No pinned nix in runtimeInputs: the client must speak the host daemon's
  # protocol/experimental-feature set (ca-derivations on hydra), so use the
  # ambient nix that just ran this app, same as chrome-vm and the updaters.
  text = ''
    # nu
    def main [...installables: string] {
      if ($installables | is-empty) {
        error make {
          msg: "usage: push-cache <installable>... e.g. push-cache .#packages.aarch64-linux.panes-guest-image"
        }
      }
      let cache_dir = (
        $env.IX_PUSH_CACHE_DIR?
        | default (
          ($env.XDG_CACHE_HOME? | default ($env.HOME | path join ".cache"))
          | path join "ix-push-cache"
        )
      )
      mkdir $cache_dir
      # zstd over the xz default: this cache lives on local disk where write
      # time, not size, is the constraint, and multi-GiB image closures under
      # xz would dominate the whole run.
      let cache_url = $"file://($cache_dir)?compression=zstd"

      for installable in $installables {
        print $"push-cache: building ($installable)"
        ^nix build --no-link $installable

        # The BUILD closure, not the runtime closure: requisites of the
        # derivation plus every already-realised output (--include-outputs
        # lists only outputs that exist, so nothing here forces extra builds).
        # That is what keeps kernel/mesa/toolchain intermediates warm across a
        # closure shift. The .drv files themselves are dropped: substitution
        # serves outputs, and drvs re-instantiate for free from the flake.
        let paths = (
          ^nix path-info --derivation $installable
          | lines
          | each {|drv| ^nix-store --query --requisites --include-outputs $drv | lines }
          | flatten
          | uniq
          | where {|p| not ($p | str ends-with ".drv") }
        )

        # --stdin instead of argv: an image build closure is thousands of
        # paths, past the execve argument limit. nix skips paths whose narinfo
        # is already in the cache, so re-runs are incremental.
        print $"push-cache: copying ($paths | length) store paths to ($cache_dir)"
        $paths | to text | ^nix copy --to $cache_url --stdin
      }

      print $"push-cache: done; substituter file://($cache_dir) is unsigned, so consumers need require-sigs = false or a separate signature"
    }
  '';
}
