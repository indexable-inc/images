const eval_jobs = "github:nix-community/nix-eval-jobs/65ebf5b7cd453a27af09cf02b1fc57b3568cc4b7"

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

  if $json {
    print ({
      base: ($base_rev | str substring 0..7)
      head: ($head_rev | str substring 0..7)
      total: $total
      changed: $changed
      added: $added
      removed: $removed
    } | to json)
  } else {
    print "<!-- blast-radius -->"
    print "### Blast radius"
    print ""
    print $"`($nc + $na)` of `($total)` checks would rebuild between base `($base_rev | str substring 0..7)` and head `($head_rev | str substring 0..7)`."
    if ($na > 0) or ($nr > 0) { print ""; print $"($na) added, ($nr) removed" }
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
