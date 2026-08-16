#!/usr/bin/env bash

# A colocated repository (`jj git init --colocate`) has both `.jj` and `.git`,
# so either fetcher can read it and both must agree on what the source tree is.
# They are compared directly here, by addressing one repository under both
# URLs and diffing the resulting store paths, because a divergence does not
# announce itself: each fetcher on its own returns a perfectly plausible tree,
# and the only symptom is that a store path moves when nothing changed.
#
# The submodule case below is why this file exists. `jj file list` reports a
# submodule as one entry naming a directory, and the workdir accessor takes
# that list as allow-list PREFIXES, so the entry admitted every file physically
# under the submodule working tree, its own `.git` pointer file included. That
# produced a tree no `git+file` fetch can produce: submodule content without
# anyone passing `submodules=1`, plus a dangling `gitdir:` pointer baked into
# the store.

source common.sh

TODO_NixOS

requireJj
requireGit

# Keep jj away from the user's config and give it a deterministic identity.
export JJ_CONFIG=$TEST_ROOT/jjconfig.toml
cat > "$JJ_CONFIG" <<EOF
[user]
name = "Nix Test"
email = "test@example.org"
EOF

hashOf() {
    nix flake prefetch --refresh --json "$1" | jq -r '.hash'
}

pathOf() {
    nix flake prefetch --refresh --json "$1" | jq -r '.storePath'
}

# Indent a captured value for a diagnostic. Parameter expansion rather than a
# pipe through `sed`, which shellcheck rejects as SC2001; the values are all
# command substitutions, so none carries a trailing newline to mis-indent.
printIndented() {
    local nl=$'\n'
    printf '  %s\n' "${1//$nl/$nl  }"
}

# Both fetchers must agree on the STORE PATH and the NAR HASH, not merely on
# "the bad artifact is gone". A fix that swaps one wrong tree for a different
# wrong tree passes any weaker assertion, and a third distinct hash for one
# working copy is the actual defect here rather than the `.git` file that made
# it visible.
#
# Reported as a tree diff, because "these two hashes differ" never answers the
# next question, which is always which files moved.
sameTree() {
    local label="$1" repo="$2" gp jp gh jh
    gp=$(pathOf "git+file://$repo"); gh=$(hashOf "git+file://$repo")
    jp=$(pathOf "jj+file://$repo");  jh=$(hashOf "jj+file://$repo")
    if [[ $gp != "$jp" || $gh != "$jh" ]]; then
        echo "git+file $gp $gh" >&2
        (cd "$gp" && find . | sort | sed 's/^/  /') >&2
        echo "jj+file  $jp $jh" >&2
        (cd "$jp" && find . | sort | sed 's/^/  /') >&2
        fail "$label: git+file and jj+file disagree on the source tree"
    fi
}

# The invariant, stated directly, rather than a check for the one artifact that
# made it visible: the fetched tree contains exactly the paths jj reports as
# tracked non-directory objects, and nothing else.
#
# It is written this way because the bug was not really "submodules". The file
# list is consumed as allow-list PREFIXES, and `CanonPath::isAllowed` grants
# access to anything under an allowed path, so ANY entry naming a directory
# licences everything physically beneath it. `describe_file_type` in jj's
# commit_templater.rs is the whole vocabulary (file, symlink, tree,
# git-submodule, conflict, and "" for absent), and of those only `tree` and
# `git-submodule` name directories. `tree` does not reach the list, because
# `jj file list` iterates `tree.entries_matching`, which descends subtrees and
# yields leaves; a submodule is the leaf jj cannot descend into. So a gitlink is
# the only way in today, and a later jj adding a type is the way in tomorrow.
# Comparing against the list itself catches both.
treeIsExactlyTracked() {
    local label="$1" repo="$2" sp expected actual
    sp=$(pathOf "jj+file://$repo")
    # Fixture paths contain no tabs or newlines, so a tab-separated template is
    # safe here and far more legible than NUL-splitting in bash.
    expected=$(jj -R "$repo" file list -T 'file_type ++ "\t" ++ path ++ "\n"' \
        | grep -E '^(file|symlink|conflict)	' | cut -f2- | sort)
    actual=$(cd "$sp" && find . \( -type f -o -type l \) | sed 's|^\./||' | sort)
    if [[ $expected != "$actual" ]]; then
        echo "jj reports as tracked:" >&2
        printIndented "$expected" >&2
        echo "the fetched tree $sp contains:" >&2
        printIndented "$actual" >&2
        fail "$label: the fetched tree is not exactly jj's tracked file set"
    fi
}

repo=$TEST_ROOT/colocated
mkdir -p "$repo/sub"
cat > "$repo/flake.nix" <<EOF
{ outputs = _: { probe = "probe"; }; }
EOF
echo alpha > "$repo/a.txt"
echo beta > "$repo/sub/b.txt"
# A symlink and a nested directory, so the exact-path allow list is exercised
# on entries that are not plain top-level files.
ln -s a.txt "$repo/link"
mkdir -p "$repo/deep/er"
echo deep > "$repo/deep/er/f.txt"
printf 'ignored-file\n' > "$repo/.gitignore"

initGitRepo "$repo" "-q -b main"
git -C "$repo" add -A
git -C "$repo" commit -qm init
jj git init --colocate "$repo"

sameTree "clean colocated tree" "$repo"
treeIsExactlyTracked "clean colocated tree" "$repo"

# A dirty tracked file. The two fetchers get there differently, git by
# digesting what differs from HEAD and jj by snapshotting into `@`, so agreeing
# here is not a given.
echo dirt >> "$repo/a.txt"
sameTree "modified tracked file" "$repo"
git -C "$repo" checkout -- a.txt

rm "$repo/sub/b.txt"
sameTree "deleted tracked file" "$repo"
git -C "$repo" checkout -- sub/b.txt

echo ignored > "$repo/ignored-file"
sameTree "gitignored file present" "$repo"
rm "$repo/ignored-file"

# git HEAD and jj `@` disagree after any plain git commit, which is the normal
# state between jj commands rather than an edge case. jj's snapshot reconciles
# it, so the trees still match.
echo gitonly > "$repo/c.txt"
git -C "$repo" add c.txt
git -C "$repo" commit -qm "git-only commit"
[[ $(git -C "$repo" rev-parse HEAD) != $(jj -R "$repo" log -r @ --no-graph -T commit_id --ignore-working-copy) ]] \
    || fail "test setup: git HEAD and jj @ were expected to differ here"
sameTree "git HEAD ahead of jj @" "$repo"

# The one place they are known NOT to agree, asserted so that it stays a
# deliberate property rather than becoming a surprise. jj auto-tracks a new
# file on snapshot; git waits for `git add`. Neither is wrong, but a colocated
# repo carrying an untracked, non-gitignored file hashes differently depending
# on which fetcher reads it.
echo brandnew > "$repo/untracked.txt"
[[ $(hashOf "git+file://$repo") != $(hashOf "jj+file://$repo") ]] \
    || fail "jj no longer auto-tracks new files, or git started including untracked ones; the routing tradeoff in flakeref.cc needs revisiting"
rm "$repo/untracked.txt"

# Submodules. A `git+file` input that does not ask for `submodules=1` renders
# the submodule absent, and jj, which cannot enumerate a submodule's own
# tracked files, must do the same rather than falling through to the raw
# filesystem.
subRepo=$TEST_ROOT/submodule-src
mkdir -p "$subRepo"
echo subcontent > "$subRepo/s.txt"
initGitRepo "$subRepo" "-q -b main"
git -C "$subRepo" add -A
git -C "$subRepo" commit -qm sub

parent=$TEST_ROOT/colocated-submodule
mkdir -p "$parent"
cat > "$parent/flake.nix" <<EOF
{ outputs = _: { probe = "probe"; }; }
EOF
echo alpha > "$parent/a.txt"
# Keep the denominator large enough that a per-file fallback cannot look like
# harmless fixed overhead. With flake.nix and the .gitmodules file below, this
# makes 31 regular files beside one gitlink.
for i in $(seq 1 28); do
    echo "contents $i" > "$parent/regular-$i.txt"
done
initGitRepo "$parent" "-q -b main"
git -C "$parent" add -A
git -C "$parent" commit -qm init
git -C "$parent" -c protocol.file.allow=always submodule add -q "$subRepo" sub
git -C "$parent" commit -qm "add submodule"
jj git init --colocate "$parent"

# Untracked junk inside the submodule working tree. Nothing tracks these, so
# nothing may copy them, and before the fix the gitlink entry admitted every
# one of them.
mkdir -p "$parent/sub/nested"
echo junk > "$parent/sub/nested/deep-junk.txt"

sameTree "colocated tree with a git submodule" "$parent"
treeIsExactlyTracked "colocated tree with a git submodule" "$parent"

# Belt and braces on the specific artifact, because a `.git` in a source tree
# is a dangling pointer into a directory that is not in the store, and the
# equality check above would pass if BOTH fetchers started leaking one.
jjTree=$(nix flake prefetch --refresh --json "jj+file://$parent" | jq -r '.storePath')
[[ ! -e $jjTree/sub/.git ]] || fail "the submodule's .git pointer file was copied into $jjTree"
[[ ! -e $jjTree/sub ]] || fail "submodule contents were included without 'submodules=1' being requested"
[[ -e $jjTree/.gitmodules ]] || fail ".gitmodules is a tracked file and should still be present"

# The working-copy fetch must read the Git snapshot even when that snapshot has
# a gitlink. The wrapper changes one working-copy file after jj has listed the
# snapshot. Reading the filesystem gets the later bytes; reading the named Git
# tree returns the snapshot. This also counts the per-file jj export calls,
# which turned a 161,368-path ix source into 161,368 processes (ENG-12220).
regularFiles=$(git -C "$parent" ls-tree -r HEAD | awk '$1 != "160000" { count++ } END { print count }')
[[ $regularFiles -eq 31 ]] \
    || fail "the working-copy fixture has $regularFiles regular files, not 31, so its traversal denominator changed"
[[ $(git -C "$parent" ls-tree HEAD sub) == 160000* ]] \
    || fail "the working-copy fixture has no gitlink, so it cannot test the omitted-gitlink fast path"

# Make a revision that no earlier sameTree call could have placed in the store.
# The wrapper snapshots this value, then replaces the disk value after listing.
echo "snapshot contents 7" > "$parent/regular-7.txt"

jjLog=$TEST_ROOT/jj-working-copy-invocations
jjMutation=$TEST_ROOT/jj-working-copy-mutated
: > "$jjLog"
mkdir -p "$TEST_ROOT/jj-working-copy-wrapper"
realJj=$(command -v jj)
cat > "$TEST_ROOT/jj-working-copy-wrapper/jj" <<WRAPPER
#!$BASH
printf '%s\n' "\$*" >> "$jjLog"
if [[ "\$*" == *" file list "* && ! -e "$jjMutation" ]]; then
    "$realJj" "\$@"
    status=\$?
    if [[ \$status -eq 0 ]]; then
        : > "$jjMutation"
        echo "after snapshot" > "$parent/regular-7.txt"
    fi
    exit \$status
fi
exec "$realJj" "\$@"
WRAPPER
chmod +x "$TEST_ROOT/jj-working-copy-wrapper/jj"

# A zero count is valid only after the wrapper and mutation both ran. Without
# these checks, a PATH mistake or changed jj command shape reports a green zero
# while testing no fast path.
"$TEST_ROOT/jj-working-copy-wrapper/jj" --version > "$TEST_ROOT/jj-working-copy-wrapper-probe"
[[ $(wc -l < "$jjLog") -eq 1 ]] \
    || fail "the working-copy jj wrapper did not record its probe invocation"
: > "$jjLog"

countPath=$(PATH=$TEST_ROOT/jj-working-copy-wrapper:$PATH pathOf "jj+file://$parent")
[[ -e $jjMutation ]] \
    || fail "the jj wrapper did not mutate the working copy after listing the snapshot, so the accessor boundary was not tested"

arrived=$(find "$countPath" -type f | wc -l)
shows=$(grep -c ' file show ' "$jjLog" || true)
diffs=$(grep -c ' diff .*file:' "$jjLog" || true)
[[ $arrived -eq $regularFiles ]] \
    || fail "a working-copy fetch reached $arrived of $regularFiles regular files after its snapshot"
[[ $(cat "$countPath/regular-7.txt") = "snapshot contents 7" ]] \
    || fail "the working-copy fetch read regular-7.txt from the changed filesystem instead of the snapshot"
[[ ! -e $countPath/sub ]] \
    || fail "a working-copy fetch rendered a gitlink that git+file omits without 'submodules=1'"
[[ $shows -eq 0 ]] \
    || fail "a working-copy fetch ran $shows 'jj file show' invocations over $regularFiles regular files and one gitlink"
[[ $diffs -eq 0 ]] \
    || fail "a working-copy fetch ran $diffs per-file 'jj diff' invocations over $regularFiles regular files and one gitlink"
echo "working-copy gitlink fast path: $arrived/$regularFiles regular files, 1/1 gitlinks omitted, $shows file-show calls, $diffs per-file diff calls"
git -C "$parent" checkout -- regular-7.txt

# Asking git for the submodule content is a different tree, so the agreement
# above is the two fetchers matching on a real file set rather than both
# returning something empty.
[[ $(hashOf "git+file://$parent?submodules=1") != $(hashOf "jj+file://$parent") ]] \
    || fail "submodules=1 produced the same tree as omitting the submodule, so the fixture has no submodule content"

# A conflicted revision is the third place the two fetchers must not agree, and
# the only one where reading Git is actively dangerous rather than merely
# different.
#
# jj represents a conflict in Git as a synthetic tree: one `.jjconflict-base-N/`
# and one `.jjconflict-side-N/` directory per input to the merge, plus a README
# explaining itself to anyone who checked it out with plain git. What sits at
# the conflicted path itself depends on the jj version -- 0.35 omits it, 0.43
# writes one side's content verbatim -- and neither is the conflict. So a
# fetcher that handed the commit to the Git accessor would produce a source
# tree that is not a conflict, not an error and not the merge: either a file
# silently replaced by one side of it, or a file that has vanished, in both
# cases beside directories nobody wrote. Nothing about it looks wrong, and it
# builds.
#
# The assertions below are written against neither shape. The invariant that
# survives a jj version bump is that the Git tree carries entries jj does not
# report as tracked, and the fetched tree carries exactly the ones it does.
conflicted=$TEST_ROOT/conflicted
jj git init "$conflicted" >/dev/null

jjc() { jj -R "$conflicted" "$@"; }

echo base > "$conflicted/a.txt"
echo untouched > "$conflicted/b.txt"
jjc describe -m base >/dev/null
conflictBase=$(jjc log -r @ --no-graph -T commit_id)

jjc new >/dev/null
echo left > "$conflicted/a.txt"
jjc describe -m left >/dev/null
conflictLeft=$(jjc log -r @ --no-graph -T commit_id)

jjc new "$conflictBase" >/dev/null
echo right > "$conflicted/a.txt"
jjc describe -m right >/dev/null
conflictRight=$(jjc log -r @ --no-graph -T commit_id)

jjc new "$conflictLeft" "$conflictRight" >/dev/null
conflictRev=$(jjc log -r @ --no-graph -T commit_id)

# Three denominators, because every assertion below this point is an absence,
# and an absence is satisfied just as well by a fixture that never conflicted
# or by a jj that spells the encoding some other way. Without these the test
# would go on passing while guarding nothing.
[[ $(jjc log -r @ --no-graph -T 'if(conflict,"1","0")') = 1 ]] \
    || fail "test setup: the fixture revision is not conflicted, so nothing below is being tested"

conflictGitPaths=$(git -C "$conflicted" ls-tree -r --name-only "$conflictRev" | sort)
conflictTracked=$(jjc file list -r "$conflictRev" -T 'path ++ "\n"' | sort)

grepQuiet '^\.jjconflict-' <<< "$conflictGitPaths" \
    || fail "jj no longer encodes a conflict as .jjconflict-* entries in its Git tree ($conflictRev); the hazard this test guards has changed shape, so re-derive it before trusting the assertions below"
[[ $conflictGitPaths != "$conflictTracked" ]] \
    || fail "the Git tree of a conflicted revision now matches jj's tracked file set exactly, so there is nothing for the fetcher to decline"

# Neither addressing mode may leak the encoding. `?rev=` and the working copy
# reach the tree by different routes, and only the second is covered by the
# equality checks earlier in this file.
conflictRevTree=$(pathOf "jj+file://$conflicted?rev=$conflictRev")
conflictWdTree=$(pathOf "jj+file://$conflicted")

for tree in "$conflictRevTree" "$conflictWdTree"; do
    got=$(cd "$tree" && find . \( -type f -o -type l \) | sed 's|^\./||' | sort)
    if [[ $got != "$conflictTracked" ]]; then
        echo "jj reports as tracked:" >&2
        printIndented "$conflictTracked" >&2
        echo "the fetched tree $tree contains:" >&2
        printIndented "$got" >&2
        echo "the backing Git tree contains:" >&2
        printIndented "$conflictGitPaths" >&2
        fail "a conflicted revision did not fetch to jj's tracked file set; the Git-side conflict encoding is reaching the store"
    fi
    grepQuiet '^<<<<<<<' "$tree/a.txt" \
        || fail "the conflicted file in $tree holds no conflict markers, so a side was silently chosen or dropped: $(cat "$tree/a.txt")"
    [[ $(cat "$tree/b.txt") = untouched ]] \
        || fail "an unconflicted file was disturbed in $tree"
done

# The two routes agree with each other, so the markers above are the fetcher's
# answer rather than one path happening to look right.
[[ $conflictRevTree = "$conflictWdTree" ]] \
    || fail "a conflicted revision fetched by rev ($conflictRevTree) and from the working copy ($conflictWdTree) produced different trees"
