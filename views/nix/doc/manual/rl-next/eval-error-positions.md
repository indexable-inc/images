---
synopsis: "An infinite recursion or stack overflow now points at the forcing site"
prs: []
---

The evaluator that arrived with the `eval-cores` setting claims a thunk by
overwriting the two words that hold its environment and expression, so by
the time it detects that a thunk is being re-entered, the position of the
thunk's own expression is gone. These errors are now reported at the place
that forced the value instead. For `let a = { } // a; in a.foo`, the
infinite recursion is reported at `a.foo` rather than at the `//` on line
2, and a stack overflow deep inside `builtins.foldl'` is reported with no
position at all rather than pointing into the list it was folding.

The trace above the error is unchanged, and still names the attribute or
operand being evaluated, so the recursive definition remains identifiable
from the frames even where the final position moved.

This applies to every evaluation, not only to `eval-cores` greater than
one, because the value representation changed unconditionally.
