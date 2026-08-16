# Deeper than a ceiling of 100 and far shallower than the default of 10000, so
# `--max-call-depth` decides whether this is 300 or an error. ENG-12540's first
# violation was the cached path ignoring that setting entirely.
let
  f = n: if n == 0 then 0 else 1 + f (n - 1);
in
f 300
