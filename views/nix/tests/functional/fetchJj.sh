#!/usr/bin/env bash

source common.sh

TODO_NixOS

requireJj

clearStoreIfPossible

# Give jj a deterministic identity and keep it from reading the user's config.
export JJ_CONFIG=$TEST_ROOT/jjconfig.toml
cat > "$JJ_CONFIG" <<EOF
[user]
name = "Nix Test"
email = "test@example.org"
EOF

repo=$TEST_ROOT/jj

jj git init "$repo" >/dev/null

echo utrecht > "$repo"/hello
mkdir "$repo"/dir
echo world > "$repo"/dir/foo

# Untracked / ignored files that must NOT end up in the store.
cat > "$repo"/.gitignore <<EOF
result
*.tmp
build/
EOF
echo junk > "$repo"/scratch.tmp
mkdir "$repo"/build
echo artifact > "$repo"/build/out

# $1: extra fields to splice into the fetchTree argument set (e.g. '; name = "foo"').
# $2: attribute to read from the result. `toString` makes this work for both
#     string attrs (outPath, rev, ref) and integer attrs (revCount, lastModified).
fetchjj() {
    nix eval --extra-experimental-features fetch-tree --impure --raw --expr \
        "toString (builtins.fetchTree { type = \"jj\"; url = \"file://$repo\"$1; }).$2"
}

# Basic fetch of the working copy. Only tracked files should be present.
path=$(fetchjj "" outPath)
[[ $(cat "$path"/hello) = utrecht ]]
[[ $(cat "$path"/dir/foo) = world ]]
[[ -e "$path"/.gitignore ]]
[[ ! -e "$path"/scratch.tmp ]]
[[ ! -e "$path"/build ]]
[[ ! -e "$path"/.jj ]]
[[ ! -e "$path"/.git ]]

# The working copy is always identified by a revision (jj has no "dirty" state).
# 40 characters because every fixture here is `jj git init`, so the ids are
# SHA-1; the length is the backend's, not the fetcher's, and a repo on jj's
# native backend reports 64 (see `parseRev`).
rev=$(fetchjj "" rev)
[[ $rev =~ ^[0-9a-f]{40}$ ]]

# revCount and lastModified are exposed.
[[ $(fetchjj "" revCount) -ge 1 ]]
[[ $(fetchjj "" lastModified) -gt 0 ]]

# Fetching again without changes is cached and yields the same path.
path2=$(fetchjj "" outPath)
[[ $path = "$path2" ]]

# Editing a tracked file changes the revision and the store path (no commit needed).
echo amsterdam > "$repo"/hello
rev2=$(fetchjj "" rev)
[[ $rev != "$rev2" ]]
path3=$(fetchjj "" outPath)
[[ $path != "$path3" ]]
[[ $(cat "$path3"/hello) = amsterdam ]]

# Adding a new file makes it tracked and visible (jj auto-tracks on snapshot).
echo new > "$repo"/dir/bar
path4=$(fetchjj "" outPath)
[[ $(cat "$path4"/dir/bar) = new ]]

# Filenames with special characters, including spaces and embedded newlines, are
# tracked and copied correctly (the file list is parsed NUL-separated).
echo spaced > "$repo/a file with spaces"
weird=$(printf 'a\nb')   # a filename containing a newline
echo nl > "$repo/$weird"
path=$(fetchjj "" outPath)
[[ $(cat "$path/a file with spaces") = spaced ]]
[[ $(cat "$path/$weird") = nl ]]

# Add an executable and a symlink, then fetch an explicit revision. The tree is
# reconstructed via the jj CLI; asserting that it yields the *same* store path as
# the working copy it was taken from verifies byte-for-byte fidelity (content,
# executable bit, symlinks and all).
chmod +x "$repo"/dir/bar
ln -s hello "$repo"/symlink
# A symlink whose target begins with '+' exercises the git-diff parser used to
# recover symlink targets (the target must not be mistaken for a diff marker).
ln -s '++/odd/target' "$repo"/pluslink
workdirPath=$(fetchjj "" outPath)
rev=$(fetchjj "" rev)
revPath=$(nix eval --extra-experimental-features fetch-tree --impure --raw --expr \
    "toString (builtins.fetchTree { type = \"jj\"; url = \"file://$repo\"; rev = \"$rev\"; }).outPath")
[[ $workdirPath = "$revPath" ]]
[[ $(cat "$revPath"/hello) = amsterdam ]]
[[ -x "$revPath"/dir/bar ]]
[[ -L "$revPath"/symlink && $(readlink "$revPath"/symlink) = hello ]]
[[ -L "$revPath"/pluslink && $(readlink "$revPath"/pluslink) = ++/odd/target ]]

# A jj input with an explicit revision is locked.
[[ $(nix eval --extra-experimental-features fetch-tree --impure --raw --expr \
    "(builtins.fetchTree { type = \"jj\"; url = \"file://$repo\"; rev = \"$rev\"; }).rev") = "$rev" ]]

# A bookmark can be fetched via `ref` and resolves to the same revision.
jj --repository "$repo" bookmark create release -r @ >/dev/null
refPath=$(nix eval --extra-experimental-features fetch-tree --impure --raw --expr \
    "toString (builtins.fetchTree { type = \"jj\"; url = \"file://$repo\"; ref = \"release\"; }).outPath")
[[ $revPath = "$refPath" ]]

# A flake in a Jujutsu workspace (which has a .jj but no .git) must be routed to
# the jj fetcher rather than the unfiltered path fetcher. This is the case the
# feature was added for.
ws=$TEST_ROOT/jj-workspace
jj --repository "$repo" workspace add "$ws" >/dev/null
cat > "$ws"/flake.nix <<'EOF'
{
  outputs = { self, ... }: {
    answer = 42;
    hasFlake = builtins.pathExists (self + "/flake.nix");
    hasScratch = builtins.pathExists (self + "/scratch.tmp");
  };
}
EOF
printf '*.tmp\n' > "$ws"/.gitignore
echo junk > "$ws"/scratch.tmp

# The flake resolves and evaluates.
[[ $(nix eval "$ws"#answer) = 42 ]]

# It was routed to the jj fetcher.
nix flake metadata "$ws" | grepQuiet "jj+file"

# And its source is filtered to tracked files only.
[[ $(nix eval "$ws"#hasFlake) = true ]]
[[ $(nix eval "$ws"#hasScratch) = false ]]

# A fresh, empty repository (the '@' commit has no files) fetches to an empty
# tree without error, and still exposes a valid revision.
empty=$TEST_ROOT/jj-empty
jj git init "$empty" >/dev/null
emptyPath=$(nix eval --extra-experimental-features fetch-tree --impure --raw --expr \
    "toString (builtins.fetchTree { type = \"jj\"; url = \"file://$empty\"; }).outPath")
[[ -d $emptyPath ]]
[[ -z $(ls -A "$emptyPath") ]]
nix eval --extra-experimental-features fetch-tree --impure --raw --expr \
    "(builtins.fetchTree { type = \"jj\"; url = \"file://$empty\"; }).rev" | grepQuiet -E '^[0-9a-f]{40}$'

# This block needs a garbage collector, and the sanitizer lane does not have
# one. It burns evaluator memory on purpose (see the calibration note below),
# and `ci/gha/tests/default.nix:57` builds that lane with
# `enableGC = !withSanitizers` because Boehm is incompatible with ASan, so none
# of those hundred million list elements is ever reclaimed. Observed as
# `fetchJj FAIL 290.26s (exit status 137 or signal 9 SIGKILL)` in run
# 30719557490, with the same test passing in 195s on a collector-having build.
#
# WHAT THIS GIVES UP, stated because a skip without it is indistinguishable
# from one added to make a red go away: on the sanitizer lane, and only there,
# nothing checks that a working-copy write during an evaluation stays out of
# that evaluation. The property is about when the fetcher snapshots, which is
# platform- and toolchain-independent, and `tests on ubuntu` plus every local
# run still cover it on the same code. So the loss is one lane's worth of
# redundancy, not the property.
#
# The number is NOT the thing to tune. Shrinking it to fit would recreate the
# exact bug the calibration note below records, where the evaluation finished
# before the writer and the test passed against the code it was written to
# catch. A green test that tests nothing is worse than a red one.
if ! evaluatorHasGC; then
    echo "fetchJj: evaluator built without a garbage collector, skipping the" \
         "working-copy mutation test; see the comment above this line" >&2
else

    # A write to the working copy during an evaluation must not reach that
    # evaluation. jj snapshots the working copy into `@` when the fetcher runs, so
    # the content of the input is decided at that moment; reading the files
    # afterwards instead would let a writer put two states into one evaluation.
    #
    # The ordering inside the evaluation is deterministic rather than raced: the
    # `probe` read forces the fetch, `slow` then burns a few seconds of pure
    # evaluation, and only then is `hello` read. The writer just has to land
    # somewhere inside that window, which is why it sleeps a fraction of a second
    # and the window is seconds long. Calibrated rather than guessed: measured on
    # an unpatched build, 30M list elements is about 1.5s of evaluation and 250M
    # about 24s, so 100M gives roughly 5s against a writer that lands at 0.5s. The
    # first version of this test used 4M and a 1s writer, so the evaluation was
    # over before the write and the test passed against the code it was written to
    # catch.
    #
    # `lazy-trees` is on deliberately: with it off the tree is copied into the
    # store during the fetch, so the second read cannot observe the working copy at
    # all and the test would pass without testing anything.
    mutation_repo=$TEST_ROOT/jj-mutation
    jj git init "$mutation_repo" >/dev/null
    echo utrecht > "$mutation_repo"/hello
    echo marker > "$mutation_repo"/marker

    expr='
      let
        src = builtins.fetchTree { type = "jj"; url = "file://'"$mutation_repo"'"; };
        probe = builtins.readFile (src + "/marker");
        slow = builtins.foldl'"'"' (a: b: a + b) 0 (builtins.genList (x: x) 100000000);
      in builtins.seq probe (builtins.seq slow (builtins.readFile (src + "/hello")))
    '

    # Wait for the evaluation to actually take its snapshot before mutating,
    # rather than guessing with a sleep. Snapshotting the working copy is a jj
    # operation, so it shows up in the operation log; polling for that turns a
    # wall-clock race (which flakes on a loaded machine, where process startup
    # alone can outlast a fixed delay) into a real happens-before.
    opCount() { jj --repository "$mutation_repo" op log --no-graph -T '"x"' 2>/dev/null | wc -c; }
    baselineOps=$(opCount)
    (
        for _ in $(seq 1 600); do
            [[ "$(opCount)" != "$baselineOps" ]] && break
            sleep 0.05
        done
        echo mutated > "$mutation_repo"/hello
    ) &
    writer=$!
    observed=$(nix eval --impure --option lazy-trees true --raw --expr "$expr")
    wait "$writer"

    [[ $observed = "utrecht" ]] || fail "an evaluation read a working copy write that happened after its snapshot: got '$observed'"

    # And the next evaluation does see it, so the first result is a snapshot rather
    # than a stale cache.
    observed=$(nix eval --impure --option lazy-trees true --raw --expr "$expr")
    [[ $observed = "mutated" ]] || fail "a later evaluation did not see the write: got '$observed'"
fi

# A by-rev fetch reads the revision out of the Git store backing the repo,
# rather than reconstructing it with one `jj file show` per file. Nothing
# cached that export either, so a pinned revision - immutable and
# content-addressed, the most cacheable input there is - was rebuilt on every
# evaluation: 16 ms per file, 238s for a 14,749-file repo (ENG-11699).
#
# Counted rather than timed. A timing threshold on a shared machine is a coin
# flip, and the count states the property directly: the export is gone, not
# merely faster.
#
# The count is checked against a denominator on purpose. "No `file show` ran"
# is also what a wrapper that never reached PATH reports, and "no export
# happened" is satisfied by fetching nothing at all, so the wrapper is required
# to have run and the tree is required to have arrived. Without those two lines
# this assertion passes loudest exactly when it has stopped testing anything.
countRepo=$TEST_ROOT/jj-count
mkdir -p "$countRepo"
initGitRepo "$countRepo" "-q -b main"
for i in $(seq 1 30); do echo "contents $i" > "$countRepo/f$i"; done

countSubRepo=$TEST_ROOT/jj-count-submodule
mkdir -p "$countSubRepo"
initGitRepo "$countSubRepo" "-q -b main"
echo submodule-content > "$countSubRepo/submodule-file"
git -C "$countSubRepo" add submodule-file
git -C "$countSubRepo" commit -qm base
git -C "$countRepo" -c protocol.file.allow=always submodule add -q "$countSubRepo" sub
git -C "$countRepo" add -A
git -C "$countRepo" commit -qm base
jj git init --colocate "$countRepo" >/dev/null
countRev=$(jj --repository "$countRepo" log -r @ --no-graph -T commit_id --ignore-working-copy)
[[ $(git -C "$countRepo" ls-tree "$countRev" sub) == 160000* ]] \
    || fail "the count fixture did not create a gitlink, so it cannot test the gitlink path"
regularFiles=$(git -C "$countRepo" ls-tree -r "$countRev" | awk '$1 != "160000" { count++ } END { print count }')
[[ $regularFiles -eq 31 ]] \
    || fail "the count fixture has $regularFiles regular files, not 31, so its traversal denominator changed"

jjLog=$TEST_ROOT/jj-invocations
: > "$jjLog"
mkdir -p "$TEST_ROOT/jj-wrapper"
realJj=$(command -v jj)
# `$BASH` rather than `/usr/bin/env bash`: the Linux build sandbox has no
# /usr/bin/env, and a shebang naming an absent interpreter fails `execve` with
# ENOENT, which `execvp` cannot distinguish from "no such file on this PATH
# entry". It moves on and runs the real jj, so the wrapper steps aside without
# a word and the counter below stays empty. That is exactly how this test first
# passed on the Linux builder while measuring nothing. `$BASH` is the
# interpreter already running this script, so it exists by construction.
cat > "$TEST_ROOT/jj-wrapper/jj" <<WRAPPER
#!$BASH
printf '%s\n' "\$*" >> "$jjLog"
exec "$realJj" "\$@"
WRAPPER
chmod +x "$TEST_ROOT/jj-wrapper/jj"

# Pin the harness before trusting what it reports. Counting an absence is only
# meaningful once the counter is known to work, and the check above is a claim
# about the sandbox that this turns into a test of it.
"$TEST_ROOT/jj-wrapper/jj" --version > "$TEST_ROOT/jj-wrapper-probe"
[[ $(wc -l < "$jjLog") -eq 1 ]] \
    || fail "the jj wrapper did not record its own invocation, so it is not on the path nix will take"
: > "$jjLog"

countPath=$(PATH=$TEST_ROOT/jj-wrapper:$PATH nix eval --extra-experimental-features fetch-tree --impure --raw --expr \
    "toString (builtins.fetchTree { type = \"jj\"; url = \"file://$countRepo\"; rev = \"$countRev\"; }).outPath")

invocations=$(wc -l < "$jjLog")
shows=$(grep -c 'file show' "$jjLog" || true)
arrived=$(find "$countPath" -type f | wc -l)

[[ $invocations -gt 0 ]] \
    || fail "the jj wrapper never ran, so the count below would hold without testing anything"
[[ $arrived -eq $regularFiles ]] \
    || fail "a by-rev fetch reached $arrived of $regularFiles regular files; 'no export ran' means nothing if the full tree did not arrive"
[[ $(cat "$countPath"/f7) = "contents 7" ]]
[[ -f $countPath/.gitmodules ]]
[[ ! -e $countPath/sub ]] \
    || fail "a by-rev fetch rendered a gitlink that git+file omits without 'submodules=1'"
[[ $shows -eq 0 ]] \
    || fail "a by-rev fetch ran $shows 'jj file show' invocations over $regularFiles regular files and one gitlink; the per-file export is still on the pinned path"

# revCount stays coherent across working-copy rewrites. An edit rewrites `@`
# in place -- new commit hash, same parents -- so the ancestor count must not
# change, and the incremental parent-count fast path (which answers this
# without re-walking the DAG) must agree with the full walk that primed the
# cache. A real new commit then adds exactly one ancestor.
rcRepo=$TEST_ROOT/jj-revcount
jj git init "$rcRepo" >/dev/null
echo one > "$rcRepo"/file
rcFetch() {
    nix eval --extra-experimental-features fetch-tree --impure --raw --expr \
        "toString (builtins.fetchTree { type = \"jj\"; url = \"file://$rcRepo\"; }).revCount"
}
rcBefore=$(rcFetch)
echo two > "$rcRepo"/file
[[ $(rcFetch) -eq $rcBefore ]] \
    || fail "revCount changed across an in-place rewrite of @"
jj --repository "$rcRepo" new >/dev/null
[[ $(rcFetch) -eq $((rcBefore + 1)) ]] \
    || fail "revCount did not grow by exactly one for a new commit"

# The workdir accessor announces `@`'s git tree hash (it serves that tree
# byte-for-byte out of the backing git store), so under the `git-hashing`
# experimental feature the mount is content-addressed by that hash instead
# of a whole-tree NAR walk. Pin the equivalence that makes this sound: the
# fetched path must equal an independent git-mode ingestion of the same
# content -- same name, same hash algorithm -- because that is exactly the
# agreement that lets a dry-run mount (lazy trees) and a later forced copy
# land on one store path.
ghRepo=$TEST_ROOT/jj-git-ca
jj git init "$ghRepo" >/dev/null
echo pinned > "$ghRepo"/file
mkdir "$ghRepo"/d
echo nested > "$ghRepo"/d/inner
ln -s file "$ghRepo"/link

ghFetch() {
    nix eval --extra-experimental-features "fetch-tree $1" --impure --raw --expr \
        "toString (builtins.fetchTree { type = \"jj\"; url = \"file://$ghRepo\"; }).outPath"
}

ghPath=$(ghFetch git-hashing)
[[ $(cat "$ghPath"/file) = pinned ]]
[[ $(cat "$ghPath"/d/inner) = nested ]]
[[ $(readlink "$ghPath"/link) = file ]]

# Independent ingestion of the same content, same name: must be the same path.
[[ $(nix store add --extra-experimental-features git-hashing --mode git --hash-algo sha1 --name source "$ghPath") = "$ghPath" ]] \
    || fail "the announced-tree-hash store path does not match an independent git-mode ingestion of the same content"

# Without the feature the same tree lands on its NAR-addressed path: the
# announcement alone must change nothing.
narGhPath=$(ghFetch "")
[[ $narGhPath != "$ghPath" ]] \
    || fail "NAR-mode and git-mode ingestion agreed on one store path, so the method switch cannot have happened"
diff -r "$narGhPath" "$ghPath"

# A content edit moves the tree hash and therefore the path, and the new
# path again matches an independent git-mode ingestion.
echo repinned > "$ghRepo"/file
ghPath2=$(ghFetch git-hashing)
[[ $ghPath2 != "$ghPath" ]]
[[ $(cat "$ghPath2"/file) = repinned ]]
[[ $(nix store add --extra-experimental-features git-hashing --mode git --hash-algo sha1 --name source "$ghPath2") = "$ghPath2" ]]

# A git-CA mount makes no narHash claim: the attribute is absent rather
# than lying about a NAR that was never computed.
[[ $(nix eval --extra-experimental-features "fetch-tree git-hashing" --impure --json --expr \
    "(builtins.fetchTree { type = \"jj\"; url = \"file://$ghRepo\"; }) ? narHash") = false ]] \
    || fail "a git-CA jj fetch still reports a narHash"

# Case-colliding names (the linux kernel's xt_CONNMARK.h / xt_connmark.h
# pattern). On a case-insensitive store volume the restore applies the
# ~nix~case~hack~ rename; the git re-hash of the restored tree must strip
# it exactly like the NAR dump does, or the copied path never matches the
# announced one. On a case-sensitive volume this passes trivially.
ccRepo=$TEST_ROOT/jj-git-ca-case
jj git init "$ccRepo" >/dev/null
echo upper > "$ccRepo"/xt_CONNMARK.h
if echo lower > "$ccRepo"/xt_connmark.h && [[ $(cat "$ccRepo"/xt_CONNMARK.h) = upper ]]; then
    ccPath=$(nix eval --extra-experimental-features "fetch-tree git-hashing" --impure --raw --expr \
        "toString (builtins.fetchTree { type = \"jj\"; url = \"file://$ccRepo\"; }).outPath")
    [[ $(nix store add --extra-experimental-features git-hashing --mode git --hash-algo sha1 --name source "$ccPath") = "$ccPath" ]] \
        || fail "git-CA round-trip of a case-colliding tree changed the store path (case hack not stripped from the git dump?)"
else
    # The checkout filesystem itself is case-insensitive, so the two
    # files collapsed before jj ever saw them; nothing to pin here.
    echo "skipping case-collision scenario: working tree is case-insensitive" >&2
fi
