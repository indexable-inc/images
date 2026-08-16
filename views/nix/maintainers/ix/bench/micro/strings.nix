# string-heavy: 20k toString + one big join, then its length
builtins.stringLength (
  builtins.concatStringsSep "," (builtins.genList (i: builtins.toString (i * 7)) 20000)
)
