# `eval-fail-path-filter-missing-ancestor.nix` through the positional
# spelling. Same machine, same question about which path the error names; the
# separate case exists because the two builtins reach it by different
# argument handling and a refusal or an error naming the wrong one of them
# sends the reader to the wrong line of their expression.
builtins.filterSource (path: type: true) /eng13123-no-such-root/sub
