# call-heavy: ~500k applications
let
  fib = n: if n < 2 then n else fib (n - 1) + fib (n - 2);
in
fib 25
