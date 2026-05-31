# Last-applied 3-way merge for a JSON config file that an app also writes to.
#
#   result = deepmerge( prune(live, last \ new), new )
#
#   * prune     drop keys we previously declared (`last`) but no longer declare
#               (`new`), compared as leaf-path sets so a key whose type changed
#               between declarations is handled without indexing errors. If the
#               live file diverged in shape at a pruned path (e.g. the app
#               replaced an object with a scalar), that path is left as-is rather
#               than failing the activation. A leaf that was the last child of an
#               object leaves an empty `{}` parent behind (harmless).
#   * deepmerge our declared keys (`new`) win; objects merge recursively, arrays
#               are atomic (a declared array replaces the live one wholesale).
#   * preserve  every other key in `live` (the app's own writes) survives.
#
# This is the kubectl-apply "last-applied" model: the previous declaration is the
# merge base, so dropping a key from Nix prunes it without clobbering app state.
# Single declarative owner only; for multiple Nix-side owners of one file you
# want per-field ownership (Server-Side Apply), which this does not implement.
#
# Inputs via --argjson: $last (previous declaration, {} if none), $live (current
# file, {} if absent), $new (desired declaration). Emits the merged object.

# Paths to "managed leaves": recurse through objects only, so arrays and scalars
# are treated as atomic leaf values (a declared array is owned as a unit).
def managedLeaves($prefix):
  to_entries[]
  | .key as $k
  | .value as $v
  | ($prefix + [$k]) as $p
  | if ($v | type) == "object" then ($v | managedLeaves($p)) else $p end;

def deepmerge($a; $b):
  if ($a | type) == "object" and ($b | type) == "object"
  then reduce ($b | keys_unsorted[]) as $k ($a; .[$k] = deepmerge($a[$k]; $b[$k]))
  else $b
  end;

# Compare managed-leaf path sets (never feeds a bad path to getpath), prune the
# paths we dropped, and leave the live shape untouched at any path delpaths
# cannot follow (try/catch) instead of aborting the activation.
( [ $last | managedLeaves([]) ] ) as $lastPaths
| ( [ $new | managedLeaves([]) ] ) as $newPaths
| ( reduce ($lastPaths - $newPaths)[] as $p
      ($live; . as $acc | try delpaths([$p]) catch $acc)
  ) as $pruned
| deepmerge($pruned; $new)
