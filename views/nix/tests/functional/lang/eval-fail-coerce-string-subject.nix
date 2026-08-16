# coerceMore stays off at these call sites, which is the other half of
# ENG-12854: a coercion that accepted every value would answer where cppnix
# raises a type error. builtins.toString is the one primop that sets it.
builtins.stringLength 42
