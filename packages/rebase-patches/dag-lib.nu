# Shared library for the patch-dependency DAG: derivation and invariant checks.
#
# One owner for the scratch-repo apply-test logic. `rebase-patches` sources this
# to regenerate `dag.json` at the end of a rebase; the `patch-dag-<name>` flake
# check sources it to verify a committed DAG is honest and in sync. Both operate
# purely on patch text against a base tree (no builds), so they stay seconds-fast.
#
# The DAG model: nodes are the `*.patch` files, an edge P -> Q means "P needs Q
# applied first to apply cleanly". The linear `NNNN` names imply every patch
# depends on all before it; in truth most are independent (e.g. six of the
# fourteen clippy patches are roots). We derive the true, sparse edge set by
# apply-tests, never by hand.

# Neutralize ambient git config that would make apply-tests non-deterministic.
# The DAG must be a pure function of the patch text and base, so a scratch repo
# used for apply-tests MUST call this once at init: rerere (recording/replaying a
# resolution mid-run silently rewrites hunks and corrupts later apply-tests), gpg
# signing (would prompt or fail with no key), and autocrlf (would renormalize
# line endings) are all forced off in the repo-LOCAL config, which overrides the
# user's ~/.gitconfig without mutating it. `rebase-patches`' own scratch repos
# call this too so a developer's global `rerere.enabled=true` cannot perturb the
# derived DAG.
export def "dag neutralize-config" [scratch: string] {
  git -C $scratch config rerere.enabled false
  git -C $scratch config rerere.autoupdate false
  git -C $scratch config commit.gpgsign false
  git -C $scratch config core.autocrlf false
}

# Seed a fresh git repo at `dest` from the read-only `src_dir` tree and commit it
# as a single base commit; returns the base rev. Used by the `patch-dag-<name>`
# check, whose base is the flake's already-fetched `flake = false` src (a
# store path with no `.git`), not a network fetch. IMPORTANT: nushell's
# `cp --recursive src/. dest` nests `src` under `dest` rather than copying its
# contents (unlike coreutils `cp`), so we copy each top-level entry explicitly,
# including dotfiles like `.gitattributes` (which affect `git apply`/`am`).
export def "dag seed-base-repo" [src_dir: string, dest: string] {
  # `ls -a` yields dotfiles too; exclude only the `.`/`..` self-links.
  for entry in (ls -a $src_dir | get name) {
    let base = ($entry | path basename)
    if $base == "." or $base == ".." { continue }
    cp --recursive $entry $dest
  }
  # Store paths are read-only (0444) and `cp` preserves that, so `git am` would
  # fail with "unable to write file ... Permission denied" mid-apply and be
  # misread as a non-applying patch. Make the working copy writable.
  chmod -R u+w $dest
  git -C $dest init --quiet
  git -C $dest config user.email "check@indexable.dev"
  git -C $dest config user.name "patch-dag check"
  dag neutralize-config $dest
  git -C $dest add --all
  git -C $dest commit --quiet --no-gpg-sign -m base
  (git -C $dest rev-parse HEAD | str trim)
}

# Prepare a scratch git repo checked out at `base`, with `am`/`apply` ready.
# `git_dir` is the scratch dir; caller fetches `base` into it first. We reset to
# a pristine `base` before every trial so apply-tests never see a dirty tree.
export def "dag reset-to-base" [scratch: string, base: string] {
  # Abort any half-finished `am` from a prior trial. `git am --abort` is the
  # normal path, but a `git am` that fails on the FIRST patch of a fresh run can
  # leave a `.git/rebase-apply` dir that `--abort` refuses to clear ("Resolve
  # operation not in progress"); a stale `rebase-apply` then makes the next
  # `git am` think a session is still open and misbehave. So we also remove it
  # directly. Then hard-reset the tree and index to a pristine base.
  do { git -C $scratch am --abort } | complete | ignore
  let rebase_apply = ($scratch | path join ".git" "rebase-apply")
  if ($rebase_apply | path exists) { rm --recursive --force $rebase_apply }
  git -C $scratch checkout --quiet --force --detach $base
  git -C $scratch reset --quiet --hard $base
  git -C $scratch clean --quiet -fdx
}

# Apply an ordered list of patch *files* onto a pristine base via `git am`.
# Returns the resulting tree hash on success, or null if any patch failed to
# apply (the failure is the signal, so we swallow it and abort cleanly). Ordering
# is the caller's NNNN order restricted to the subset; `git am` is exact-only
# here (no 3-way), because a DAG edge is precisely "this text does not apply
# without that ancestor".
export def "dag apply-order" [scratch: string, base: string, patch_files: list<string>] {
  dag reset-to-base $scratch $base
  for f in $patch_files {
    let r = (do { git -C $scratch am $f } | complete)
    if $r.exit_code != 0 {
      do { git -C $scratch am --abort } | complete | ignore
      return null
    }
  }
  (git -C $scratch rev-parse "HEAD^{tree}" | str trim)
}

# Does `patch` apply cleanly given ONLY the ordered `ancestors` (paths), applied
# to base? Builds the ancestor stack with `git am`, then `git apply --check` for
# the candidate. Returns false if the ancestor stack itself fails to build (an
# unapplicable subset can never be a valid ancestor set), matching the semantics
# the DAG derivation needs: "applies given exactly these ancestors".
export def "dag applies-with" [scratch: string, base: string, ancestors: list<string>, patch: string] {
  dag reset-to-base $scratch $base
  for f in $ancestors {
    let r = (do { git -C $scratch am $f } | complete)
    if $r.exit_code != 0 {
      do { git -C $scratch am --abort } | complete | ignore
      return false
    }
  }
  let checked = (do { git -C $scratch apply --check $patch } | complete)
  $checked.exit_code == 0
}

# Derive the sparse dependency DAG for an ordered `*.patch` list against `base`.
#
# `patches` is a record list `[{name, file}]` in NNNN order (name = bare file
# name, file = absolute path). Returns `[{patch, deps}]` in NNNN order, deps a
# list of patch names, transitively reduced (only DIRECT edges).
#
# Algorithm (n is tiny, so O(n^2) apply-checks are milliseconds; exhaustive is
# fine): for each patch i, start from its full predecessor set (which must apply,
# since that is the committed serialization) and greedily drop the lowest-index
# ancestor whose removal keeps patch i applying. This yields a locally-minimal
# SUFFICIENT ancestor set (it stays downward-closed by construction, because
# dropping an ancestor that another retained ancestor still needs would break the
# stack and be rejected). Then reduce to direct edges: an ancestor already
# implied by another dep's own closure is not a direct edge. Prefers the sparsest
# DAG on ties, per the design.
export def "dag derive" [scratch: string, base: string, patches: list<record>] {
  let names = ($patches | get name)
  let files = ($patches | get file)
  let n = ($patches | length)

  # full_anc.(i) = minimal sufficient ancestor set (as sorted index list) for
  # patch i. Built with an immutable fold so no closure captures a `mut`.
  let full_anc = (
    0..<$n | reduce --fold [] {|i, acc|
      let patch_file = ($files | get $i)
      # `seq` yields an actual list of ints (a range does not); empty for i == 0.
      let preds = (if $i == 0 { [] } else { seq 0 ($i - 1) })
      # Greedy transitive reduction: repeatedly try to drop the lowest kept
      # ancestor; if the patch still applies without it, drop it and restart the
      # pass, until no single drop preserves applicability.
      let keep = (
        generate {|keep|
          let dropped = (
            $keep | sort | reduce --fold null {|q, found|
              if $found != null { $found } else {
                let trial = ($keep | where {|x| $x != $q })
                let trial_files = ($trial | sort | each {|j| $files | get $j })
                if (dag applies-with $scratch $base $trial_files $patch_file) { $trial } else { null }
              }
            }
          )
          if $dropped == null { {out: $keep} } else { {next: $dropped} }
        } $preds
        | last
      )
      $acc | append [($keep | sort)]
    }
  )

  # Transitive reduction to direct edges. q is a direct dep of i iff q is in
  # full_anc.i and q is NOT in the union of full_anc over the other members of
  # full_anc.i (i.e. no retained ancestor already pulls q in).
  0..<$n | each {|i|
    let anc = ($full_anc | get $i)
    let implied = ($anc | each {|r| $full_anc | get $r } | flatten | uniq)
    let direct = ($anc | where {|q| $q not-in $implied } | sort)
    {
      patch: ($names | get $i)
      deps: ($direct | each {|j| $names | get $j })
    }
  }
}

# Render the DAG document for `dag.json`. `base` is the upstream rev the DAG was
# derived against (so a stale committed DAG is obvious), `nodes` the derived
# edges. Field order is stable so regeneration is byte-deterministic.
export def "dag document" [base: string, nodes: list<record>] {
  {
    # Committed as data next to the patches; regenerated automatically by
    # `rebase-patches` and verified in sync by the `patch-dag-<name>` check.
    comment: "Auto-generated by `nix run .#rebase-patches`; do not edit by hand. Nodes are patch files, deps are the minimal set of ancestors each patch needs to apply. See patch-dag-design."
    base: $base
    nodes: $nodes
  }
}

# Serialize the DAG document to the exact bytes we commit: pretty JSON, two-space
# indent, trailing newline. Both the writer and the in-sync check must agree on
# these bytes, so this is the single formatting owner.
export def "dag to-json" [doc: record] {
  ($doc | to json --indent 2) + "\n"
}

# --- Invariant verification (the `patch-dag-<name>` flake check) --------------

# Verify the three DAG invariants against a committed `dag.json`, plus topo
# order. Pure text work on `base` in `scratch`. `patches` is the NNNN-ordered
# `[{name, file}]` list; `doc` is the parsed committed dag.json. Returns a record
# `{ok: bool, errors: list<string>}`; the caller decides how to exit.
export def "dag verify" [scratch: string, base: string, patches: list<record>, doc: record] {
  mut errors = []
  let names = ($patches | get name)
  let file_of = ($patches | reduce --fold {} {|p, acc| $acc | insert $p.name $p.file })
  let dag_names = ($doc.nodes | get patch)

  # (0) node set matches the patch files exactly.
  let missing = ($names | where {|nm| $nm not-in $dag_names })
  let extra = ($dag_names | where {|nm| $nm not-in $names })
  if ($missing | is-not-empty) {
    $errors = ($errors | append $"dag.json is missing nodes for patches: ($missing | str join ', ')")
  }
  if ($extra | is-not-empty) {
    $errors = ($errors | append $"dag.json has nodes for nonexistent patches: ($extra | str join ', ')")
  }
  # (0b) declared base matches the base we are checking against.
  if $doc.base != $base {
    $errors = ($errors | append $"dag.json base ($doc.base) does not match the pinned base ($base)")
  }

  # Only run structural checks when the node set is coherent.
  if ($missing | is-empty) and ($extra | is-empty) {
    let deps_of = ($doc.nodes | reduce --fold {} {|nd, acc| $acc | insert $nd.patch $nd.deps })
    let pos = ($names | enumerate | reduce --fold {} {|it, acc| $acc | insert $it.item $it.index })

    # (c-topo) NNNN order is a valid topological order of the DAG: every dep of a
    # patch appears earlier in the NNNN sequence. Also rejects unknown dep names.
    for nm in $names {
      for d in ($deps_of | get $nm) {
        if $d not-in $names {
          $errors = ($errors | append $"patch ($nm) declares unknown dependency ($d)")
        } else if ($pos | get $d) >= ($pos | get $nm) {
          $errors = ($errors | append $"NNNN order is not topological: ($nm) depends on later patch ($d)")
        }
      }
    }

    # (a) every patch applies given ONLY its declared DAG ancestors (transitive
    # closure of deps), applied in NNNN order. Catches undeclared dependencies.
    for nm in $names {
      let anc = (dag closure $deps_of $nm)
      let anc_files = ($anc | where {|x| $x in $pos } | sort-by {|x| $pos | get $x } | each {|x| $file_of | get $x })
      let patch_file = ($file_of | get $nm)
      if not (dag applies-with $scratch $base $anc_files $patch_file) {
        $errors = ($errors | append $"patch ($nm) does NOT apply given only its declared ancestors [($anc | str join ', ')]: undeclared dependency")
      }
    }

    # (b) every pair of DAG-INDEPENDENT patches commutes: A-then-B and B-then-A
    # both apply and yield byte-identical trees. Fast path: if the two patches
    # touch disjoint file sets they trivially commute, so only overlapping-file
    # pairs pay the two-order apply. Independence = neither is in the other's
    # transitive closure.
    let footprints = ($names | reduce --fold {} {|nm, acc| $acc | insert $nm (dag footprint ($file_of | get $nm)) })
    for i in 0..<($names | length) {
      for j in ($i + 1)..<($names | length) {
        let a = ($names | get $i)
        let b = ($names | get $j)
        let a_anc = (dag closure $deps_of $a)
        let b_anc = (dag closure $deps_of $b)
        let dependent = ($b in $a_anc) or ($a in $b_anc)
        if $dependent { continue }
        # Disjoint footprints trivially commute.
        let fa = ($footprints | get $a)
        let fb = ($footprints | get $b)
        let overlap = ($fa | where {|f| $f in $fb })
        if ($overlap | is-empty) { continue }
        # Overlapping files: prove both orders succeed and agree byte-for-byte.
        # Apply against each patch's shared ancestor closure first so the pair
        # applies in isolation, then the two orders.
        let shared = (($a_anc | append $b_anc) | uniq)
        let shared_files = ($shared | where {|x| $x in $pos } | sort-by {|x| $pos | get $x } | each {|x| $file_of | get $x })
        let ab = (dag apply-order $scratch $base ($shared_files | append [($file_of | get $a)] | append [($file_of | get $b)]))
        let ba = (dag apply-order $scratch $base ($shared_files | append [($file_of | get $b)] | append [($file_of | get $a)]))
        let overlap_str = ($overlap | str join ", ")
        if $ab == null or $ba == null {
          $errors = ($errors | append $"DAG-independent patches ($a) and ($b) do not both apply in either order; overlapping files [($overlap_str)]. They are not independent.")
        } else if $ab != $ba {
          $errors = ($errors | append $"DAG-independent patches ($a) and ($b) overwrite each other: applying A-then-B and B-then-A give different trees; overlapping files [($overlap_str)].")
        }
      }
    }
  }

  {ok: ($errors | is-empty), errors: $errors}
}

# Transitive closure of a patch's ancestors (all patches it transitively depends
# on), from a `{patch: deps}` map. Excludes the patch itself.
export def "dag closure" [deps_of: record, patch: string] {
  mut seen = []
  mut stack = ($deps_of | get $patch)
  while ($stack | is-not-empty) {
    let cur = ($stack | first)
    $stack = ($stack | skip 1)
    if $cur in $seen { continue }
    $seen = ($seen | append $cur)
    if $cur in ($deps_of | columns) {
      $stack = ($stack | append ($deps_of | get $cur))
    }
  }
  $seen | uniq
}

# The set of repo-relative file paths a patch touches (its hunk footprint), read
# from the patch's `diff --git a/<path> b/<path>` headers. Used by the commute
# fast path to skip disjoint pairs.
export def "dag footprint" [patch_file: string] {
  open --raw $patch_file
  | lines
  | where {|l| $l | str starts-with "diff --git a/" }
  | each {|l| $l | parse "diff --git a/{a} b/{b}" | get a.0? | default "" }
  | where {|p| $p != "" }
  | uniq
}
