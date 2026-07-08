# Single source of truth for the ix public binary cache identity.
#
# `cache.ix.dev` is the ix pull-through cache: ncps fronting the `ix-public`
# atticd cache, falling through to cache.nixos.org for generic nixpkgs paths so
# one substituter covers both. atticd signs every narinfo it serves server-side
# under `ix-workspace:`, so this one trusted key verifies both ix's builds and
# index's published packages.
#
# Any module that adds `url` as a substituter MUST also trust `publicKey`.
# `require-sigs` is on in the guests, so an untrusted substituter is rejected
# narinfo-by-narinfo as unsigned and the daemon silently falls back to building
# the whole closure from source.
#
# Consumed through `specialArgs.ix.cache` and the public `index.lib.cache`. The
# flake's own `nixConfig` (flake.nix) has to repeat these as literals because
# Nix reads `nixConfig` before the flake's lib exists; keep the two in sync.
{
  url = "https://cache.ix.dev";
  publicKey = "ix-workspace:JuAaeOPfR3GL3nUICpEz/88/+S3BzGF3L6bPYFy0GwI=";
}
