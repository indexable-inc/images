# `builtins.filterSource` is `builtins.path` with the arguments spelled
# positionally, and until this case the corpus exercised only the attribute
# spelling (eval-okay-path.nix). Both drive one filtered-copy machine, so what
# this pins is the entry point: the filter arrives as the first argument and
# the root as the second, and the store object takes its name from the root's
# base name rather than from an explicit `name`.
#
# The filter accepts exactly one top-level entry, so the result does not move
# as the corpus around it grows -- every other name is rejected, and a
# rejected directory is not descended into.
builtins.filterSource (path: _: baseNameOf path == "data") ./.
