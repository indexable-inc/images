# The other direction from eval-okay-underscore-digit-separators: an
# underscore is a digit separator only BETWEEN two digits, so a trailing one,
# one before a letter, and one after the exponent marker all stay outside the
# literal and start an identifier instead.
#
# Each case is written as a list rather than an application, because list
# elements juxtapose without applying: if the lexer is right, `[ 1_ ]` holds
# the integer 1 and the value of `_`, and if the lexer swallowed the
# separator it would hold one element instead of two. An over-eager lexer
# fails these on the count, before any value is compared.
let
  _ = "trailing";
  _x = "letter";
  e_5 = "exponent";
  _1_000 = "leading";
in
[
  ([ 1_ ] == [ 1 "trailing" ])
  ([ 1_x ] == [ 1 "letter" ])
  ([ 1e_5 ] == [ 1 "exponent" ])
  ([ _1_000 ] == [ "leading" ])
  (builtins.length [ 1_ ] == 2)
  (builtins.length [ 1_000 ] == 1)
]
