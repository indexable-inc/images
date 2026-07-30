# Render the commit message, or the pull request body, for a flake relock: a
# subject line followed by every flake input whose locked revision the relock
# moved, with the old and new revision (ENG-11408).
#
# Args:
#   --slurpfile before <lock>   flake.lock as the base branch has it
#   --slurpfile after  <lock>   flake.lock as the candidate commit has it
#   --arg subject <line>        first line of the message
#   --arg named "<a b c>"       inputs the subject already names, so the suffix
#                               can call out the ones it does not
#
# Inputs are named by their path from the root (`nix-src`, `index/nix-src`),
# never by the lock's internal node key: keys carry dedup suffixes
# (`nixpkgs_2`) that renumber whenever the graph changes, so keying on them
# would report a renumbering as a move and hide a real move behind a rename. A
# node that no path reaches falls back to its raw key rather than being
# dropped, so nothing can go unreported.
#
# Transitive inputs are reported, not just the root's own. That is not
# thoroughness for its own sake: the evaluator pin this was written for is
# `index/nix-src` in ix's lock, two levels down, so a report restricted to root
# inputs names nothing at all for the commits that prompted it.
#
# Comparing parsed revisions rather than lines is what makes the report survive
# a lock file that was reformatted or whose nodes were reordered with no input
# moving.
#
# The body always says something. A report whose quiet state is an empty list
# is satisfied by a report that is simply broken, so "nothing moved" is a
# sentence here, never an absence.

# Node key -> input path, breadth first from the root so each node gets its
# shortest path. `follows` edges are arrays rather than node keys and are
# skipped: they retarget an input at a node that some string edge already
# reaches, so skipping them costs no coverage.
def input_paths:
  . as $lock
  | ($lock.nodes // {}) as $nodes
  | ($lock.root // "root") as $root
  | {seen: {($root): ""}, frontier: [$root]}
  | until(.frontier | length == 0;
      reduce .frontier[] as $node ({seen: .seen, frontier: []};
        (.seen[$node]) as $parent
        | reduce (($nodes[$node].inputs // {}) | to_entries[]) as $edge (.;
            $edge.value as $child
            | if ($child | type) != "string" or (.seen | has($child))
              then .
              else
                .seen[$child] = (if $parent == "" then $edge.key
                                 else $parent + "/" + $edge.key end)
                | .frontier += [$child]
              end)))
  | .seen;

# Input path -> locked revision. narHash stands in for an input pinned without
# a rev (a tarball, a bare path), so moving one of those is reported too rather
# than reading as "did not move".
def locked_revs:
  . as $lock
  | input_paths as $paths
  | [($lock.nodes // {}) | to_entries[]
     | select(.value.locked != null)
     | {key: ($paths[.key] // .key),
        value: (.value.locked.rev // .value.locked.narHash // "unpinned")}]
  | from_entries;

($before[0] // {} | locked_revs) as $old
| ($after[0] // {} | locked_revs) as $new
| [((($old | keys) + ($new | keys)) | unique)[]
   | {input: ., old: ($old[.] // "absent"), new: ($new[.] // "absent")}
   | select(.old != .new)] as $moved
| ($named | split(" ") | map(select(. != ""))) as $expected
| [$moved[].input | select(. as $input | ($expected | index($input)) == null)] as $unnamed
| (if ($unnamed | length) == 0 then ""
   elif ($unnamed | length) <= 2 then ", relock " + ($unnamed | join(", "))
   else ", relock " + ($unnamed[0:2] | join(", "))
        + " and " + ((($unnamed | length) - 2) | tostring) + " more"
   end) as $suffix
| [$subject + $suffix, ""]
  + (if ($moved | length) == 0
     then ["No locked input revision changed. flake.lock may still differ from",
           "the base branch in metadata (narHash, lastModified) only."]
     else [(if ($moved | map(.input | contains("/")) | any)
            then "Flake input revisions this bump moves (a/b means input b of input a):"
            else "Flake input revisions this bump moves:" end),
           ""]
          + [$moved[] | "- \(.input): \(.old) -> \(.new)"]
     end)
| .[]
