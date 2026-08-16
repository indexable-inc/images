# hashstring.nix scaled from 45000 outer iterations to 450, so a measurement
# finishes. Same shape, same mix of `builtins.<name>` references per unit of
# work, 1/100th of the wall clock.
builtins.foldl' (a: b: a + b) 0 (
  builtins.genList (
    x:
    builtins.foldl' (p: q: p + q) 0 (
      builtins.genList (
        y: builtins.stringLength (builtins.hashString "sha512" (toString (x * 1000 + y)))
      ) 1000
    )
  ) 450
)
