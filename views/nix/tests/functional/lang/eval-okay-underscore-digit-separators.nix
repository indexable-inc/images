# Underscore digit separators in numeric literals are purely visual: they are
# admitted between digits only and stripped before the value is parsed. A
# leading underscore still starts an identifier, so `_1_000` is a variable.
let
  _1_000 = 7;
in
[
  (1_000 == 1000)
  (1_000_000 == 1000000)
  (1_0_0 == 100)
  (1__0 == 10)
  (9_223_372_036_854_775_806 == 9223372036854775806)
  (1_000.000_1 == 1000.0001)
  (0.000_100 == 0.0001)
  (.5_0 == 0.50)
  (2.5e1_0 == 2.5e10)
  (6.674_30e-1_1 == 6.67430e-11)
  (_1_000 == 7)
]
