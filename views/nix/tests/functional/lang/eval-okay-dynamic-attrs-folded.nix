# `${"literal"}` is not a dynamic attribute. `AttrName::visit`
# (parser-state.hh:91) folds an ExprString back to a symbol, so the parser
# never records one -- which is why the two rules that reject dynamic
# attributes outright, `let` (parser.y:271) and `inherit` (parser.y:529), let
# all four of these through. NixOS/nix#14642.
let
  ${"a"} = 1;
  outer = {
    ${"b"} = 2;
  };
  inherit (outer) ${"b"};
in
{
  inherit ${"a"};
  fromSet = b;
  # Folding happens per component, so a nested path folds too.
  nested = ({ x.${"y"} = 3; }).x.y;
  # And the folded name is a name, so `?` and `.` see it.
  has = outer ? ${"b"};
}
