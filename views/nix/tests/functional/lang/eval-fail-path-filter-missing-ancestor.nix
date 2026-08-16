# A filtered `builtins.path` whose root is not there, and neither is the
# directory above it.
#
# The interesting half is the ANCESTOR, not the root. cppnix resolves the
# root's symlinks before it copies (`primops.cc:2977`), and
# `SourceAccessor::resolveSymlinks` walks every component with `maybeLstat`
# (`source-accessor.cc:91`): a component it cannot see is nullopt, which is
# recorded as `absent` and is simply not a symlink. The failure then comes out
# of the copy, and names the ROOT.
#
# The Rust backend runs that walk itself, because its filter is a Nix function
# and the copy is a question for the embedder. Asking a question that threw on
# a missing component made it report the first ancestor instead --
# `/eng13123-no-such-root` here -- and under pure eval, where the accessor
# knows only `/nix/store`, that first ancestor is `/nix` for every store path
# in existence. 90 of ix's 144 flake attributes died on `path '/nix' does not
# exist` before their filter ran once (ENG-13123).
#
# So this case is about WHICH path the error names. Both arms must name the
# root. It is `eval-fail` rather than `eval-okay` because the corpus cannot
# reach the pure-eval shape at all: `--pure-eval` forbids reading the corpus
# file itself, so the oracle arm never starts. The pure-eval reproducer is a
# flake, and lives in `maintainers/ix/rust-nix-eval-gate.sh` section 8c.
builtins.path {
  path = /eng13123-no-such-root/sub;
  name = "x";
  filter = path: type: true;
}
