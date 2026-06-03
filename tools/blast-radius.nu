const eval_jobs = "github:nix-community/nix-eval-jobs/65ebf5b7cd453a27af09cf02b1fc57b3568cc4b7"

# Caps that keep the rendered Mermaid graph under GitHub's ~50KB / ~25-node
# budget: only the highest fan-out root causes, and only a few affected checks
# per cause, are drawn (the comment still lists every changed check in full).
const max_causes = 6
const max_checks_per_cause = 5

def eval-checks [repo: string, rev: string] {
  let flakeref = $"git+file://($repo)?rev=($rev)&allRefs=1#checks.x86_64-linux"
  # nix-eval-jobs sits at the head of the pipeline, and Nushell only
  # propagates the last command's status, so a startup/lock/fetch failure
  # would otherwise be swallowed and leave us parsing empty rows. Wrap it
  # like the `check` wrapper so a nonzero exit aborts here.
  let rows = (
    do --capture-errors {
      ^nix run $eval_jobs -- ...[
        "--flake" $flakeref
        "--workers" "8"
        "--option" "accept-flake-config" "true"
        "--option" "eval-cache" "false"
      ]
    }
    | lines
    | each {|l| if (($l | str trim) | is-not-empty) { $l | from json } }
    | compact
  )
  let errors = ($rows | each {|r| if ($r.error? | is-not-empty) { $"($r.attr? | default '?'): ($r.error)" } } | compact)
  if ($errors | is-not-empty) {
    for e in $errors { print --stderr $"eval error @($rev): ($e)" }
    error make { msg: $"checks failed to evaluate at ($rev)" }
  }
  # nix-eval-jobs quotes any path segment that needs quoting in Nix source
  # (dots, leading digits, etc.), so attrs like `image-minecraft_1.21.11-fabric`
  # arrive as `"image-minecraft_1.21.11-fabric"`. Strip the surrounding quotes
  # so the diff, the rendered comment, and the workflow's safename regex all
  # see the bare attribute name.
  $rows | each {|r| { attr: ($r.attr | str trim --char '"'), drvPath: $r.drvPath } }
}

def drv-for [tbl, name] {
  $tbl | each {|r| if ($r.attr == $name) { $r.drvPath } } | compact | first
}

# A check's category for the v1 breakdown: the segment before the first dash
# (`image-foo` -> `image`, `rust-test-bar` -> `rust`, `lint` -> `lint`). Coarse
# but it groups the families that actually share fan-out.
def category [name: string] {
  let head = ($name | split row "-" | first)
  if ($head | is-empty) { $name } else { $head }
}

# Strip a store path to its derivation name: drop the `/nix/store/<hash>-`
# prefix and the `.drv` suffix, leaving e.g. `ix-rust-workspace` or
# `search_core-0.1.0` -- a stable, readable label for a graph node.
def drv-name [p: string] {
  $p | path basename | str replace -r '^[a-z0-9]{32}-' '' | str replace -r '\.drv$' ''
}

# v2 root causes: for each changed check, the *direct* input derivations whose
# store path differs between base and head. Direct references suffice because a
# changed transitive dependency propagates a fresh hash up to the immediate
# input, so the check's direct refs always move when anything below them does --
# and querying references (not the full closure) keeps this cheap. Causes are
# ranked by fan-out (how many checks they rebuild) and capped for the graph.
def causes-for [b, h, changed: list<string>] {
  mut acc = {}
  for name in $changed {
    let hd = (drv-for $h $name)
    let bd = (drv-for $b $name)
    if ($hd | is-empty) or ($bd | is-empty) { continue }
    let changed_names = (
      do --ignore-errors {
        let hr = (^nix-store -q --references $hd | lines | where ($it | str ends-with ".drv"))
        let br = (^nix-store -q --references $bd | lines | where ($it | str ends-with ".drv"))
        $hr | where ($it not-in $br) | each {|p| drv-name $p } | uniq
      }
    )
    for cn in ($changed_names | default []) {
      $acc = ($acc | upsert $cn (($acc | get -o $cn | default []) | append $name))
    }
  }
  $acc
  | transpose name checks
  | each {|r| { name: $r.name, checks: ($r.checks | uniq | sort), fanout: ($r.checks | uniq | length) } }
  | sort-by fanout --reverse
  | first $max_causes
  | each {|r| { name: $r.name, checks: ($r.checks | first $max_checks_per_cause) } }
}

def main [base?: string, head?: string, --json] {
  let repo = (^git rev-parse --show-toplevel | str trim)
  let head_rev = (^git rev-parse --verify $"($head | default 'HEAD')^{commit}" | str trim)
  let base_in  = (^git rev-parse --verify $"($base | default 'origin/main')^{commit}" | str trim)
  let base_rev = (^git merge-base $base_in $head_rev | str trim)

  let b = (eval-checks $repo $base_rev)
  let h = (eval-checks $repo $head_rev)
  let bn = ($b | get attr)
  let hn = ($h | get attr)

  let changed = ($h | each {|r| if ($r.attr in $bn) and ((drv-for $b $r.attr) != $r.drvPath) { $r.attr } } | compact | sort)
  let added   = ($hn | each {|a| if (not ($a in $bn)) { $a } } | compact | sort)
  let removed = ($bn | each {|a| if (not ($a in $hn)) { $a } } | compact | sort)
  let total = ($hn | length)
  let nc = ($changed | length)
  let na = ($added | length)
  let nr = ($removed | length)

  # v1: changed + added grouped by category, largest first.
  let categories = (
    ($changed ++ $added)
    | each {|n| category $n }
    | uniq --count
    | rename name count
    | sort-by count --reverse
  )
  # v2: root-cause fan-out edges (only meaningful for the changed set).
  let causes = (if ($nc > 0) { causes-for $b $h $changed } else { [] })

  if $json {
    print ({
      base: ($base_rev | str substring 0..7)
      head: ($head_rev | str substring 0..7)
      total: $total
      changed: $changed
      added: $added
      removed: $removed
      categories: $categories
      causes: $causes
    } | to json)
  } else {
    print "<!-- blast-radius -->"
    print "### Blast radius"
    print ""
    print $"`($nc + $na)` of `($total)` checks would rebuild between base `($base_rev | str substring 0..7)` and head `($head_rev | str substring 0..7)`."
    if ($na > 0) or ($nr > 0) { print ""; print $"($na) added, ($nr) removed" }
    if ($categories | length) > 0 {
      print ""
      print "```mermaid"
      print "pie showData title Rebuilt checks by category"
      for c in $categories { print $"  \"($c.name)\" : ($c.count)" }
      print "```"
    }
    if ($causes | length) > 0 {
      print ""
      print "```mermaid"
      print "flowchart LR"
      mut i = 0
      for c in $causes {
        let cid = $"c($i)"
        print $"  ($cid)\([\"($c.name)\"]\)"
        mut j = 0
        for k in $c.checks {
          print $"  ($cid) --> ($cid)k($j)\([\"($k)\"]\)"
          $j = $j + 1
        }
        $i = $i + 1
      }
      print "```"
    }
    if ($nc > 0) {
      print ""
      print "<details><summary>changed checks</summary>"
      print ""
      for a in $changed { print $"- ($a)" }
      print ""
      print "</details>"
    }
  }
}
