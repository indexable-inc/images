# list + arithmetic: 100k-element genList folded strictly
builtins.foldl' (a: b: a + b) 0 (builtins.genList (i: i) 100000)
