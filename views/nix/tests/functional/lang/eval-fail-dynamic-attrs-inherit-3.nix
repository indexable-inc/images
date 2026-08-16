# The `inherit` inside a `rec` set takes the same rule as the two next to it
# (eval-fail-dynamic-attrs-inherit{,-2}): `attrs` throws at parse time
# (parser.y:529), so the binding never has to be demanded for it to fire --
# and `x` below is never demanded. A separate case because a rec set compiles
# its inherits through a different path than a plain set, and a backend can
# reject one and accept the other.
rec {
  inherit ${"a" + ""};
  x = 1;
}
