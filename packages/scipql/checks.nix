# scipql end-to-end acceptance (#3898), imported by lib/per-system.nix into
# the per-system check catalog.
{
  mkCheck,
  scipql,
}: {
  # End-to-end proof that scipql resolves SCIP monikers and acts only on
  # the right symbol, exercising all three surfaces (query / fix /
  # rename) of the real pipeline. The wrapped CLI bakes rust-analyzer +
  # the pinned toolchain + souffle; the fixture is a dependency-free
  # crate with a `net::Socket` and a same-named `mock::Socket`, so
  # rust-analyzer's `cargo metadata` needs no network. Tree-sitter
  # (astlog) could not tell the two `Socket`s apart; this is the
  # semantic-disambiguation guarantee.
  scipql-e2e = mkCheck "scipql-e2e" {
    nativeBuildInputs = [scipql];
    script = ''
      export HOME="$TMPDIR/home"
      mkdir -p "$HOME"
      cp -r ${
        builtins.path {
          name = "scipql-two-sockets-fixture";
          path = ./tests/fixtures/two-sockets;
        }
      } work
      chmod -R u+w work
      cd work
      fail=0

      scipql index . -o index.scip

      # query: the two same-named structs resolve to distinct monikers.
      # (printf, not a heredoc: a heredoc terminator would not sit at
      # column 0 after Nix strips the indented string's indentation.)
      printf '%s\n' \
        '.decl sockets(sym:symbol)' \
        '.output sockets' \
        'sockets(s) :- occurrence(s, _, _, _, "definition"), symbol_info(s, _, "Socket").' \
        > sockets.dl
      q=$(scipql query index.scip sockets.dl)
      echo "$q" | grep -q 'net/Socket#' || { echo "query: missing net/Socket# definition" >&2; fail=1; }
      echo "$q" | grep -q 'mock/Socket#' || { echo "query: missing mock/Socket# definition" >&2; fail=1; }

      # fix: the replacement text is COMPUTED in datalog (cat + a join to
      # the display name), not a constant, and still scoped to net by moniker.
      printf '%s\n' \
        'edit(path, start, end, cat("Net", name)) :-' \
        '  occurrence(sym, path, start, end, _),' \
        '  symbol_info(sym, _, name),' \
        '  substr(sym, strlen(sym) - strlen("net/Socket#"), strlen("net/Socket#")) = "net/Socket#".' \
        > netname.dl
      d=$(scipql fix index.scip netname.dl)
      echo "$d" | grep -q 'NetSocket' || { echo "fix: datalog-computed replacement (cat) did not apply" >&2; fail=1; }
      echo "$d" | grep -q 'src/mock.rs' && { echo "fix: computed edit wrongly touched mock.rs" >&2; fail=1; }

      # rename: apply to disk, then assert the net struct + its reference
      # changed while mock::Socket and the net struct's own fd field did not.
      scipql rename index.scip 'net/Socket#' Stream --write
      grep -q 'pub struct Stream' src/net.rs || { echo "rename: net::Socket was not renamed" >&2; fail=1; }
      grep -q 'net::Stream' src/lib.rs || { echo "rename: the net::Socket reference was not renamed" >&2; fail=1; }
      grep -q 'pub struct Socket' src/mock.rs || { echo "rename: mock::Socket was wrongly changed" >&2; fail=1; }
      grep -q 'pub fd: i32' src/net.rs || { echo "rename: the struct's own fd field was wrongly renamed" >&2; fail=1; }

      if [ "$fail" != 0 ]; then
        echo "--- net.rs ---" >&2; cat src/net.rs >&2
        echo "--- mock.rs ---" >&2; cat src/mock.rs >&2
        echo "--- lib.rs ---" >&2; cat src/lib.rs >&2
        exit 1
      fi
    '';
  };
}
