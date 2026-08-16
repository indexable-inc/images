# builtins-reference-heavy: 400k executions of one `builtins.<name>` reference.
#
# The paired file builtins-ref-hoisted.nix is the same loop with the reference
# lifted into a `let`, so the difference between the two is the cost of the
# reference itself and nothing else. ENG-12539.
builtins.foldl' (acc: _: acc + builtins.stringLength "abcdefgh") 0 (builtins.genList (i: i) 400000)
