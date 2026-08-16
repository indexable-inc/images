# POSIX bracket-expression members the regex crate reads as syntax:
# `\` (escape), `[` (nested class), `&`/`~` (set-operator halves), and the
# two first positions where `]` is a member. ENG-13140 follow-up: the first
# pattern is the class validating every ix shell abbreviation.
[
  (builtins.match ''[^|;&<>()$`'"\[:space:]]+'' "gc")
  (builtins.match "[a\\\\]+" "a\\a")
  (builtins.match "[a[]+" "a[a")
  (builtins.match "[]a]+" "]a")
  (builtins.match "[^]a]+" "bc")
  (builtins.match "[a&b]+" "a&b")
  (builtins.match "[a~b]+" "a~b")
  (builtins.match "[a[:digit:]]+" "a12")
  (builtins.match "[a&b]+" "c")
  (builtins.split "[x[]" "a[bxc")
]
