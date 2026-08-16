# The store directory is hashed into this path rather than merely prefixed
# onto it, so the same expression under two stores is two different answers
# that look equally plausible. ENG-12541 is exactly this being cached as one.
(builtins.derivationStrict {
  name = "cachesem";
  system = "x86_64-linux";
  builder = "/bin/sh";
}).out
