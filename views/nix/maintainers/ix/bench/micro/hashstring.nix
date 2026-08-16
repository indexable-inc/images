# The rung E expression, kept here so that section of
# maintainers/ix/rust-default-ladder.md has an input rather than a quotation.
# Read the numbers there, not here: they are re-measured as the evaluator
# moves, and a copy in this comment would be wrong within the week. What this
# file promises is only that it is the same expression, reflowed by nixfmt and
# otherwise unchanged, evaluating to 5760000000 on both backends. Dominated by
# hashString and foldl', with 45M `builtins.<name>` references in it.
#
# The outer count is the knob. At 45000 both backends take tens of seconds;
# hashstring-small.nix is the same shape at 450 and is the one to reach for.
builtins.foldl' (a: b: a + b) 0 (
  builtins.genList (
    x:
    builtins.foldl' (p: q: p + q) 0 (
      builtins.genList (
        y: builtins.stringLength (builtins.hashString "sha512" (toString (x * 1000 + y)))
      ) 1000
    )
  ) 45000
)
