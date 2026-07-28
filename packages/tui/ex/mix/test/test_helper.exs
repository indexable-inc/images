# support/eventually.exs is staged next to this file by the ex target of
# unibind.build (packages/unibind/nix/ex.nix); the suite only runs inside
# the assembled mix package, never straight from this source tree.
Code.require_file("support/eventually.exs", __DIR__)
ExUnit.start()
