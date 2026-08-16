# shellcheck shell=bash
# shellcheck disable=SC2034  # every value here is read by a gate that sources this file
# Checked-in expected counts for the differential gates. Sourced, not run.
#
# Why a file rather than a literal in each gate: every one of these numbers
# was once a slack floor picked to be comfortably below what the gate had
# been observed to produce (`served > 30` against an observed 51), and a
# floor with 40% of headroom cannot see a regression that eats a third of
# the coverage. Recorded exactly, a change in coverage is a diff in this
# file with a commit message, which is the point -- the number moving is the
# signal, and it must not be possible to move it by accident.
#
# ## Which comparison each number gets, and why
#
# EXACT (-eq) for anything whose input is a fixed list checked in beside the
# gate: drv-parity.sh's CASES array and rust-nix-eval-gate.sh's sections are
# source code, so their counts are deterministic and any movement is a
# deliberate edit here.
#
# RATCHET (>= or <=) for anything measured against the lang corpus, which
# gains cases from upstream merges. A ratchet still refuses a regression;
# it just does not have to be touched when the corpus grows in a direction
# that only helps.
#
# ## Updating one of these
#
# Re-run the gate, read the RESULT line, and change the number in the same
# commit as the change that moved it. Do not widen a comparison to make a
# run pass: a `>=` where this file says `-eq` is how the slack came back.
#
# ## Recount immediately before you merge, and say where from
#
# GATE_RATCHETS_MEASURED_AT below is the revision every number here was
# measured at, and each gate prints it on its RESULT line. It is not
# decoration: these counts move whenever the Rust VM gains a construct, so a
# value minted against a stale tip is wrong in one of two directions. Too low
# and the floor understates what the VM can already do, so a regression back
# to the old number passes. Too high and the next merge is blocked by a
# number nobody can reproduce.
#
# This has already bitten once in one afternoon: rebasing onto the
# search-path and currentSystem work moved three of these (match 118 -> 119,
# unimplemented 46 -> 45, round-trip skips 9 -> 7) and every gate would still
# have passed on the pre-rebase numbers, quietly hiding the improvement.
#
# So: rebase onto the tip you are actually merging into, re-run the gates,
# and update this file and the revision together in that last commit.

# The revision these were measured at, and the host. Printed by every gate
# that reads this file, so a RESULT line always carries the provenance of the
# numbers it was judged against.
#
# 694a3faed is the revision the runs happened at. The commit carrying this
# file is one later and differs from it in this file alone -- a stamp naming
# its own commit cannot be written, since writing it changes the hash. Every
# number below was re-measured there in one sitting, all eight gates.
#
# ENG-13123 (the ancestor-kind fix) moved four numbers below and deliberately
# did NOT restamp, for the reason the paragraph above gives: the stamp asserts
# a full sweep, and that branch re-ran four gates, not eight. Restamping it to
# 18a64b27a would have made every gate print that revision as the provenance
# of numbers nobody measured there -- including the four this file carries for
# fetch-tree-parity and rust-incremental-gate, which were last measured at
# 694a3faed and still are. The numbers that did move name their own revision
# and host in the comment above each of them, which is the narrower and true
# claim. Restamp on the next full sweep.
GATE_RATCHETS_MEASURED_AT=694a3faed
GATE_RATCHETS_MEASURED_ON=dev-compute-3

# -- drv-parity.sh ---------------------------------------------------------
# cases: the length of the CASES array. Exact, because a case list that
# silently shrinks is the failure the count exists to catch.
DRV_PARITY_CASES=64
# match: how many pairs agreed byte for byte. A ratchet, because the Rust
# backend is still implementing derivationStrict and this number climbs.
# Without it, a backend that refuses every case scores unimplemented=60
# mismatch=0 and exits 0.
# Raised 40 -> 42 on the re-measure at 694a3faed. Not this branch's doing:
# the two cases came from work merged between 36ea3f8ff and the fork tip this
# branch sits on. Raised anyway, because a floor two below the current state
# is a floor that cannot see those two cases regress -- which is what the
# header of this file says not to leave lying around quietly.
DRV_PARITY_MIN_MATCH=46

# -- fetch-parity.sh -------------------------------------------------------
# cases: the length of the CASES array. Exact, for the reason
# DRV_PARITY_CASES is exact.
FETCH_PARITY_CASES=32
# produced: pairs where BOTH arms printed a /nix/store path. Exact, and it is
# the only number on that gate's RESULT line that two identically-failing arms
# cannot satisfy -- a broken fixture, a store that lost the fixture paths, or a
# backend that refuses every fetch all score mismatch=0 without it. Measured on
# dev-compute-3; see the revision above.
FETCH_PARITY_PRODUCED=10
# status-differs: pairs that failed alike but exited with different statuses.
# Exact, and not zero: cppnix gives a fixed-output hash mismatch exit status
# 102 (`.withExitStatus(102)`) and the Rust arm's fetch hook has no channel to
# carry one, so it exits 1. Three cases hit it -- the explicit zero hash, the
# empty sha256 that becomes the zero hash, and the same under tryEval.
# ENG-12719. Measured on dev-compute-3.
FETCH_PARITY_STATUS_DIFFERS=3

# -- fetch-tree-parity.sh --------------------------------------------------
# cases: the length of that gate's CASES array. Exact.
FETCH_TREE_PARITY_CASES=36
# produced: pairs where both arms computed a VALUE (not just failed alike).
# The one number two identically-failing arms cannot satisfy. Exact.
FETCH_TREE_PARITY_PRODUCED=26
# unimplemented: EXACT, and deliberately not zero. Two shapes are refused by
# name -- a bare string/path argument to fetchTree or fetchGit, and
# `publicKeys` -- because serving either would mean this crate reimplementing
# URL parsing or a JSON writer that decides a store path. Three cases exercise
# them. Exact in both directions: a new refusal is coverage leaving the gate,
# and closing one of these must move the number deliberately.
FETCH_TREE_PARITY_REFUSED=3

# -- rust-nix-eval-gate.sh -------------------------------------------------
# Every case here is a literal in the script, so all four are exact.
#
# 67 -> 70, 53 -> 56, 29 -> 32 with section 8c, the ENG-13123 regression case:
# three `serves` rows for a filtered `builtins.path`, a `builtins.filterSource`
# and the unfiltered control, all inside a flake and therefore under pure eval.
# Three new pairs, all three producing a value on both arms, so all three
# counts move by three and `refused` does not move at all.
#
# They are here rather than in the lang corpus because that corpus cannot hold
# them: `--pure-eval` on a corpus file makes the ORACLE arm refuse to read the
# file, so the case never reaches an evaluator. A flake is the only shape that
# gets pure eval and a readable source at once, which is why 90 of ix's 144
# flake attributes could fail on this for as long as they did with every gate
# green.
#
# Measured at 18a64b27a on dev-compute-5, not at the GATE_RATCHETS_MEASURED_AT
# above; see the note beside that stamp for why it was not moved.
RUST_NIX_EVAL_PAIRS=70
RUST_NIX_EVAL_SERVED=56
RUST_NIX_EVAL_PRODUCED=32
RUST_NIX_EVAL_REFUSED=3

# -- lang-diff.sh ----------------------------------------------------------
# unimplemented is a bucket that neither passes nor fails, so a case landing
# in it looks handled (ENG-12438). Capping it makes the bucket a budget:
# a construct that stops being implemented has to move this number. The
# ladder's eventual gate is 0. Measured 45 on dev-compute-3 at 36ea3f8ff.
# It had only ever gone down -- 50, then 46 once the .flags fix moved four
# cases out of the bucket and into real comparisons, 45 with search-path and
# currentSystem, 41 when builtins resolution moved to compile time
# (ENG-12539), 37 once unsafeGetAttrPos started answering null -- and then
# ENG-12569 moved it back up by 8, deliberately. See the note under
# LANG_DIFF_MIN_MATCH.
# 44 as of the hello.outPath work: `eval-okay-search-path.nix` stopped being
# unimplemented when `<nix/fetchurl.nix>` started resolving (ENG-12607).
# 40 as of the fetcher work (694a3faed, dev-compute-3), down from 44.
# Three of the four are attributable and one is not:
#   * eval-fail-fetchurl-baseName, -attrs and -attrs-name call
#     `builtins.fetchurl`, which had no table entry before this branch, so
#     they could only have been in this bucket. Re-run filtered, they are now
#     `pairs=3 fail-as-fail=3 unimplemented=0`.
#   * the fourth came from work merged between 36ea3f8ff and the fork tip.
#     No baseline run was done at the tip, so it is not attributed here.
#
# 39 once the tree fetchers landed, and this one is attributable by reading:
# `eval-fail-fetchTree-negative.nix` passes `owner = -1`, which is now the
# evaluator's own "negative value given for 'fetchTree' argument 'owner'"
# rather than an unimplemented builtin. Its sibling
# `eval-fail-fetchTree-relative-path.nix` passes a bare string, which this
# backend refuses by name, so it stays in the bucket -- deliberately.
#
# 38 with builtins.parseDrvName (ENG-12746), and the case is named:
# `eval-okay-versions.nix` calls it, so on the base it could only be in this
# bucket. Filtered runs on the two binaries, both built on dev-compute-2 from
# the same base, say so directly -- base `--only eval-okay-versions` is
# `pairs=1 match=0 unimplemented=1`, branch is `pairs=1 match=1
# unimplemented=0`.
#
# 28 with the refusal retirement (ENG-13082), down from 40. Twelve pairs left
# the bucket and every one is named; four of them are corpus pairs this branch
# adds, so the count is decomposed against a base measured over the SAME
# 278-pair corpus rather than against the 273 the base tree ships:
#
#   rec { __overrides = ...; }   eval-okay-overrides, eval-okay-attrs6,
#                                eval-okay-inherit-from,
#                                eval-okay-overrides-dynamic (new),
#                                eval-fail-set-override,
#                                eval-fail-overrides-dynamic-dup (new)
#   ~/... path literals          eval-okay-home-path-warn,
#                                eval-okay-home-path (new),
#                                eval-okay-path-string-interpolation
#   dynamic name in inherit      eval-fail-dynamic-attrs-inherit,
#                                eval-fail-dynamic-attrs-inherit-2,
#                                eval-fail-dynamic-attrs-inherit-3 (new)
#
# Enumerated by running both binaries over every corpus file and diffing which
# ones printed `rust-eval unimplemented`, not by arithmetic. Seven are
# eval-okay and move into `match`; five are eval-fail and move into
# `fail-as-fail`, which is why match rises by 7 and not by 12.
#
# eval-fail-home-path-fatal stayed in the bucket deliberately for a while:
# its `.flags` set `--lint-absolute-path-literals fatal`, which the command
# layer refused via `refusalTokens::parserLint` before the crate saw the
# source. That was the lint gap (ENG-12597), closed below at the 24 -> 4.
# 27 with the underscore digit separators fix (ENG-13119): the rust arm lexes
# them now, so eval-okay-underscore-digit-separators leaves this bucket and
# lands in `match`. See the LANG_DIFF_MIN_MATCH note below for the paired
# measurement both numbers come from.
#
# 4 with the mechanism burndown, from 24. Five mechanisms, twenty cases, and
# the enumeration is measured, not arithmetic: both binaries -- base
# e64631c27 and branch 55ea4ceba, built on dev-compute-6 -- ran every corpus
# file and the two "printed `rust-eval unimplemented`" lists were diffed.
# Twelve leavers are eval-okay and move into `match` (the MIN_MATCH note
# below is the paired measurement), eight are eval-fail and move into
# `fail-as-fail`, which no ratchet reads:
#
#   parser lints (warn/fatal)    eval-okay-dotdotslash-abs-fatal,
#     as Diagnose settings       eval-okay-dotdotslash-path-fatal,
#     (closes ENG-12597)         eval-okay-dotslash-abs-fatal,
#                                eval-okay-dotslash-path-fatal,
#                                eval-okay-url-literal-quoted-fatal,
#                                eval-fail-abs-path-fatal,
#                                eval-fail-home-path-fatal,
#                                eval-fail-short-path-literal,
#                                eval-fail-url-literal
#   pipe-operators feature       eval-fail-pipe-operators
#   --xml --no-location          eval-okay-closure, eval-okay-functionargs,
#                                eval-okay-xml
#   parseFlakeRef /              eval-okay-parse-flake-ref,
#     flakeRefToString           eval-okay-flake-ref-to-string,
#                                eval-fail-flake-ref-to-string-negative-integer
#   parse-toml-timestamps        eval-okay-fromTOML-timestamps,
#                                eval-fail-fromTOML-timestamps
#   builtins.toPath              eval-okay-pathexists, eval-fail-to-path
#
# eval-okay-pathexists earns a warning label: implementing toPath let the
# case run to completion for the first time, and the first full run scored a
# MISMATCH on its trailing-slash rows -- cppnix's `prim_pathExists` demands a
# directory when a string argument ends in `/` or `/.` (primops.cc:2105), and
# the rust arm normalized the slash away. That is `NeedPath::DirExists` now,
# and the moral is the ledger's usual one: a case leaving this bucket is not
# a case passing, it is a case being COMPARED for the first time.
#
# The four that remain are named mechanisms, each a real subsystem and none a
# one-off: eval-okay-autoargs (--argstr/--arg auto-calling needs a new entry
# ABI), eval-okay-import (scopedImport needs a compiler scope frame and a
# modcache key that carries it), eval-fail-fetchTree-relative-path (the
# fetchTree STRING form, a wire widening), eval-fail-toJSON-non-utf-8
# (non-UTF-8 source and values, a subsystem-scale decision).
LANG_DIFF_MAX_UNIMPLEMENTED=4
# match is byte-identical stdout on an eval-okay pair -- the only outcome
# that proves the two evaluators agree about a value, as opposed to agreeing
# about a failure class. Measured 118 of 259 pairs on dev-compute-3 at
# 36ea3f8ff.
#
# ## This floor went DOWN, on purpose, and that needs saying
#
# It was 123. ENG-12569 made the Rust backend refuse by name when a path or
# URL literal lint is set to `fatal`, because it has no parser lint and was
# otherwise happily evaluating programs cppnix rejects. That took mismatch
# from 3 to 0 -- the divergence is gone -- and cost five eval-okay cases
# which set such a lint and then use the form it permits (`./foo`, a quoted
# URL). cppnix accepts those and this backend agreed with it; they are
# refused anyway, because telling "the lint is set" from "the lint would
# fire" means implementing the lint.
#
# Three wrong answers traded for five refusals is the right trade --
# `unimplemented` is a named gap and `mismatch` is a wrong answer -- but it
# IS coverage leaving the gate, and both of these numbers moving the wrong
# way at once is exactly what this file exists to stop anyone doing quietly.
# Implementing the lints in rnix recovers all five (ENG-12597).
# 119 for the same reason the line above moved: the same corpus case now
# matches rather than refusing.
# Left at 119 while the corpus grew by one matching pair, which is a floor
# that now hides an improvement -- exactly what the paragraph above says not to
# do quietly, so it is said here. `eval-okay-concatstringssep-coerce` was added
# with the coercion fix (ENG-12628) and matches: verified as byte-identical
# output from `nix-instantiate --eval --strict` and from the crate, but NOT
# through lang-diff's own two arms, which need a built nix on a dev node. Move
# this to 120 on the next measured run rather than taking my word for it; a
# number in this file that nobody measured is worse than a stale one, because
# the staleness is at least visible in the revision it names.
# 122 as of 694a3faed on dev-compute-3, up from 119. NOT from this branch:
# the three fetchurl corpus cases it moved are eval-fail pairs, which land in
# fail-as-fail and never in `match`. The +3 is other merged work, and it is
# recorded rather than left as headroom for the reason above.
# 122 -> 123 with __curPos (ENG-12713): eval-okay-curpos was the last entry in
# the semantic-divergence half of eval-allowlist.toml that the rust arm failed
# outright, so it moved from allowlisted to match and allowlisted went 8 -> 7.
# Re-measured on hydra rather than dev-compute-3, against this branch's tip;
# the baseline arm of the same comparison, built from 8968fcc2d in the same
# tree, produced match=122 allowlisted=8.
# 125 with parseDrvName (ENG-12746). Both of the two are this branch's and both
# are named, which is why this moves rather than being left as headroom:
#
#   * `eval-okay-parse-drv-name`, added here. `--only` on that one pair is
#     `pairs=1 match=1 unimplemented=0`.
#   * `eval-okay-versions`, which already called `parseDrvName` and so was
#     `unimplemented` before. See the note on LANG_DIFF_MAX_UNIMPLEMENTED.
#
# Measured on dev-compute-2, not dev-compute-3, and NOT as part of a re-measure
# of all eight gates -- so GATE_RATCHETS_MEASURED_AT above is deliberately left
# where it was rather than restamped to this branch, which would claim the
# other six numbers had been re-taken here. Two binaries built in one sitting
# from one base (ix-patched 98d0536604c60935a87ea090f44109cd08caaedd), with
# `-Dnix:rust-eval=enabled`, and the gate's own sha256 field distinguishes
# them:
#
#   base    0f0e4e71bb2534d84184f3b8d88ea65b28e91b868b0a5b710e7c0e4f9b5c9195
#           pairs=261 match=123 mismatch=0 unimplemented=39 allowlisted=7
#   branch  pairs=262 match=125 mismatch=0 unimplemented=38 allowlisted=7
#
# The base arm reproducing 123 and 39 exactly, on a different node from the one
# the file names, is worth recording on its own: those two numbers were still
# right.
# 124 with the coercion class fix (ENG-12854), and this number is what the
# merged tree scores rather than what it ought to score. Decomposition:
#
#   123  at ix-patched 71f1ecc7e, measured
#   +1   eval-okay-coerce-string-subject, the pair this change adds
#   ---
#   124
#
# The pair is attributed by a filtered run on the branch binary:
# `--only 'eval-*coerce-string-subject'` gives `pairs=2 corpus=264 match=1
# fail-as-fail=1 mismatch=0`. Its sibling `eval-fail-coerce-string-subject`
# lands in fail-as-fail and never in `match`, which is why the corpus grows by
# two and this number by one.
#
# Measured on hydra, aarch64-darwin, NOT on a dev node. Two binaries built in
# one tree from ix-patched 71f1ecc7e with `-Dnix:rust-eval=enabled
# --buildtype=debugoptimized`, differing in the Rust library alone, and the
# gate's own sha256 field distinguishes them:
#
#   base    4118a63f4bdb081d3b843adf9cb7236b32288f49715c9a3bb8f10684421c0daf
#           pairs=262 match=123 mismatch=3 unimplemented=38 allowlisted=7
#   branch  bc87c27c94f0673b08393a63d0880e5415edab4700748d0d7aca82debe00ac89
#           pairs=264 match=124 mismatch=3 unimplemented=38 allowlisted=7
#
# ## Why this is 124 and not the 125 it replaces
#
# 125 was measured on dev-compute-2 (Linux) at 98d0536604, which is before
# #132 (ENG-12792, the read hooks routed through cppnix's rootFS, merged
# 2026-08-06 12:12Z). Three pairs mismatch identically on the base and on the
# branch here, all of them that change's subject matter:
#
#   eval-okay-readDir-symlinked-directory
#   eval-okay-symlink-resolution
#   eval-fail-readDir-not-a-directory-2
#
# Two of the three are eval-okay pairs, so they account arithmetically for the
# gap between 125 and 123. **Consistent, not proven**: those two numbers were
# taken on different platforms as well as different revisions, so nothing here
# rules out darwin contributing. Restoring them is ENG-12871, and when that
# lands the tree scores 126; per the standing rule, whichever of the two
# changes merges second writes that number, and both comments carry the full
# decomposition so a reader can audit it without re-measuring.
#
# The floor is deliberately NOT set to 126 in advance. A floor above what the
# merged tree scores is a gate that fails on its own merge, which is a
# different thing from a ratchet.
#
# 133 with the bridge's symlink resolution (ENG-12871), which is the number
# the comment above predicted as 126 before this branch grew its corpus. The
# +9 over 124 is decomposed and every part is measured on the branch binary
# with `--only`, so no part of it is arithmetic:
#
#   124  what the merged tree scores, per the block above
#   +2   the two eval-okay rows that were MISMATCH. Filtered over all three
#        reported rows: `pairs=3 match=2 fail-as-fail=1 mismatch=0`. The third,
#        eval-fail-readDir-not-a-directory-2, is an eval-fail pair, so it moves
#        into fail-as-fail and never into match.
#   +7   the new eval-okay pairs, all matching:
#        `pairs=7 match=7 fail-as-fail=0 mismatch=0` over readFile-symlink,
#        readFile-symlinked-ancestor, readDir-symlinked-ancestor,
#        pathExists-dangling-symlink, pathExists-symlinked-ancestor,
#        readFileType-symlink, import-symlinked-directory.
#   ---
#   133
#
# fail-as-fail moves 91 -> 94 by the same accounting: the class fix above plus
# two new eval-fail pairs, `pairs=2 match=0 fail-as-fail=2 mismatch=0` over
# readFile-dangling-symlink and readFileType-symlinked-ancestor.
#
# mismatch is 0 for the first time since #132.
#
# ## Two measurements, on two bases, and only one of them is mine
#
# The base number here is 124, taken from the block above rather than
# re-measured: ENG-12854 measured it on its own branch, which is the tree this
# rebased onto. What I measured is the branch, plus the same fix on the
# PREVIOUS base, which is where the before/after pair with two binaries lives:
#
#   ix-patched e96dccb3a, both -Dnix:rust-eval=enabled debugoptimized, hydra
#   base    14a63d15b214bfef7a4fc8ad10a8ae8b1bf941ab1c0673e06ef97a046b01f2a6
#           pairs=262 match=123 fail-as-fail=90 mismatch=3   -> exit 1
#   branch  6c34119d035f07102ed58d02dd9195f0fe9ff5f69e3464bcb6f05e2b091bea74
#           pairs=271 match=132 fail-as-fail=93 mismatch=0   -> exit 0
#
#   rebased onto 4fcb25698 (ENG-12854 merged), same flags, same machine
#   branch  066ffad456c768c579d87042fef1bd0d2cdd3cfd8f2acb411873a9930cd34f18
#           pairs=273 match=133 fail-as-fail=94 mismatch=0   -> exit 0
#
# The +1 between the two branch runs is ENG-12854's own corpus pair, not
# anything of this one's.
#
# ## The floor was unmet on the merged tree, and this is what repairs it
#
# 125 stood while ix-patched itself scored 123, from #132 (ENG-12792, the
# bridge reads) until ENG-12854 lowered it to 124 and wrote down why. Both of
# those numbers were the gate describing a divergence rather than a floor. 133
# is the first since #132 that the tree actually meets.
#
# Measured on hydra (aarch64-darwin), not on a dev-compute node, so
# GATE_RATCHETS_MEASURED_AT above is deliberately not restamped.
#
# The fault this repairs is platform-independent by reading, not by
# measurement: the resolution the bridge skipped lives in
# `EvalState::realisePath` (primops.cc:172), `SourceAccessor::resolveSymlinks`
# (source-accessor.cc:91) and `resolveExprPath` (eval.cc:3423), and the refusal
# it ran into is `PosixSourceAccessor::assertNoSymlinks`
# (posix-source-accessor.cc:192). None of the four is under a `__linux__` or
# `__APPLE__` conditional; the only platform split in those files is `_WIN32`.
# A Linux re-measure is still owed before 133 is treated as the Linux number.
#
# 143 with the refusal retirement (ENG-13082), and the whole +10 is measured
# on two binaries over one corpus rather than inferred:
#
#   135  ix-patched 928208c80 over the corpus AS SHIPPED (273 pairs), hydra,
#        sha256 307caa36c61339acad1ddd7d33b0090766422e5224ce0090f05c8d5c4b9ed90a.
#        Two above the 133 floor this replaces; that headroom is merged work,
#        not this branch's, and is being written down here rather than left.
#   +1   eval-okay-dynamic-attrs-folded, one of the six pairs this branch
#        adds. The base already MATCHES it -- `${"literal"}` folding was
#        implemented before this branch -- so it is a regression guard over
#        the rewritten `static_attr_name`, not a retirement.
#   ---
#   136  the same base binary over this branch's corpus. Measured, not
#        arithmetic: the five other new pairs are refused or fail-as-fail on
#        the base, so only this one moves the number.
#   +7   the seven eval-okay pairs that left `unimplemented`, listed under
#        LANG_DIFF_MAX_UNIMPLEMENTED above.
#   ---
#   143
#
# fail-as-fail moves 94 -> 99 by the same accounting: the five eval-fail pairs
# in that list.
#
# mismatch is 0 on both arms of the comparison, so nothing was traded for this.
#
# Measured on hydra (aarch64-darwin), NOT on a dev-compute node, so
# GATE_RATCHETS_MEASURED_AT above is deliberately not restamped -- the other
# gates' numbers were not re-taken here. Both binaries built in their own
# worktree from ix-patched 928208c80 with `-Dnix:rust-eval=enabled
# --buildtype=debugoptimized`; the gate's sha256 field distinguishes them and
# both RESULT lines are quoted in the PR.
#
# The base half of that pair was NOT re-measured after merging ix-patched
# forward to 722f21904 (#177, #178), so the 136 is a number from the older
# base. What was measured is the branch, twice, on two binaries:
#
#   a4a895e2631299a2348c02edb6699ae0e9ac5f83b4e6ea6fe6bc3d91949d1089  pre-merge
#   15427b7c554ac937a3d6b8700a2745cefd9ed01bc7898c5bb8dd2c69211f09b1  post-merge
#
# Both `pairs=278 match=143 fail-as-fail=99 mismatch=0 unimplemented=28`,
# identical in every field. So the four merged commits moved nothing this gate
# measures, and the 136 is still the right base -- which is a measurement of
# the merge's effect rather than a reading of which files it touched.
# 148 with the underscore digit separators fix (ENG-13119).
#
# Measured as a pair on dev-compute-6, both binaries built in their own tree
# with `-Dnix:rust-eval=enabled --buildtype=debugoptimized`, over the SAME
# 279-pair corpus -- the base tree got this branch's two corpus files copied
# in, so the two runs differ in the evaluator and in nothing else:
#
#   base   ix-patched 43e574aa8   sha256 1f5efa0d0b55999d8d89bb45dac17cb2e4487187b3091fa0b2af57f0058fc7bb
#     pairs=279 match=146 fail-as-fail=99 mismatch=1 unimplemented=28 allowlisted=4  (rc=1)
#   branch            c1e36228c   sha256 d31ae5b8be31b9f42afba33a0baceb98dcb594181a9a5e482f58e1b0c6bf1e9c
#     pairs=279 match=148 fail-as-fail=99 mismatch=0 unimplemented=27 allowlisted=4  (rc=0)
#
# The delta is exactly this branch's two corpus files, and it is worth reading
# which bucket each left:
#
#   eval-okay-underscore-digit-separators             unimplemented -> match
#   eval-okay-underscore-digit-separator-boundaries   MISMATCH      -> match
#
# The second one is the point. `compile_apply`'s refusal only ever fired for an
# Apply node, so a separated literal anywhere else was not refused, it was got
# WRONG: on the base binary `[ 1_000 ]` is `[ 1000 ]` on the cpp arm and
# `error: undefined variable '_000'` on the rust arm. A refusal-token census
# cannot see that, and the ENG-13119 sweep did not, because no attribute it
# reached happened to use the shape.
#
# What was NOT re-measured: the base's own 146 is 3 above the 143 recorded
# here, and those 3 belong to merges since 694a3faed (#184, #186, #187) which
# did not restamp this file. They are not decomposed further, and
# GATE_RATCHETS_MEASURED_AT above is deliberately left alone for the same
# reason the hydra note above gives -- the other six gates' numbers were not
# re-taken in this sitting, only lang-diff's pair and drv-parity's.
#
#
# 144 with the ENG-13123 corpus cases, and the +1 is one named case:
# `eval-okay-filterSource`, the corpus's first `builtins.filterSource` of any
# kind. Measured `pairs=281 corpus=281 match=144 fail-as-fail=101 mismatch=0
# unimplemented=28 allowlisted=7` at 18a64b27a on dev-compute-5 (not at the
# GATE_RATCHETS_MEASURED_AT above), against 278/143/99 on the
# same tree without the three new files.
#
# The other two new cases are `eval-fail`, so they land in `fail-as-fail`
# (99 -> 101) and move no floor -- which is worth saying, because they are the
# two that actually exercise the fix. With the fix reverted and the three
# files kept, a filtered run scores `pairs=3 match=1 mismatch=2`: the two
# eval-fail cases name the ancestor where cppnix names the root, and
# `eval-okay-filterSource` is unaffected. Neither ratchet here can see that,
# which is the general shape of the hole ENG-13123 fell through -- a wrong
# ANSWER on a case nobody had written moves no count.
#
# Then 144 -> 147 on merging ix-patched forward, and the +3 is not this
# branch's. PR #186 (ENG-12137, positions through the Rust evaluator) deleted
# exactly three ids from eval-allowlist.toml -- `eval-okay-getattrpos`,
# `eval-okay-getattrpos-functionargs` and `eval-okay-inherit-attr-pos` -- and
# those three cases moved from `allowlisted` to `match`. Both halves are
# visible in one RESULT line: allowlisted 7 -> 4 as match 144 -> 147. It is
# the same allowlisted-becomes-match mechanism recorded above at the 8 -> 7.
#
# Raised anyway rather than left at 144, because 144 is exactly the number the
# header warns about: a floor minted before the merge would still have passed
# after it, hiding an improvement that three cases' worth of upstream work
# paid for. Measured `pairs=281 corpus=281 match=147 fail-as-fail=101
# mismatch=0 unimplemented=28 allowlisted=4 skipped=1` at a95f6f78b on
# dev-compute-5.
#
# ## Both of the above landed, so the floor is their sum
#
# 148 (ENG-13119, above) + 1 (`eval-okay-filterSource`, ENG-13123) = 149.
# The two lines do not overlap -- one is a lexer gaining a literal
# shape, the other is the corpus gaining its first `builtins.filterSource`
# -- and only ENG-13123's `eval-okay-*` case is a `match`; its three
# `eval-fail-*` cases land in `fail-as-fail`, which no ratchet reads.
#
# `unimplemented` takes ENG-13119's 27 unchanged: nothing on this branch
# moves it, since a filtered `builtins.path` was never unimplemented -- it
# was answered wrongly, which is the distinction that let it hide.
#
# Measured, and the sum above was written down as a prediction before the run
# rather than read off it afterwards: `pairs=283 corpus=283 match=149
# fail-as-fail=102 mismatch=0 crash=0 unimplemented=27 allowlisted=4
# skipped=1` at d0965b61a on dev-compute-5. 283 = upstream's 279 plus this
# branch's four files.
#
# 153 as of ENG-13139/ENG-13144: convertHash and hashFile flipped
# eval-okay-convertHash, eval-okay-hashfile and eval-fail-hashfile-missing
# out of `unimplemented` (24 there, from 27).
#
# 154 as of ENG-13146: eval-okay-hashfile-binary joins the corpus. It
# matches only while hashFile digests the file's raw bytes; the lossy-UTF-8
# digest this ratchet buries scored a mismatch on the pre-fix binary.
#
# 166 with the mechanism burndown: the twelve eval-okay leavers named in the
# LANG_DIFF_MAX_UNIMPLEMENTED note above (24 -> 4), every one re-run as a
# real comparison and every one byte-identical. Measured `pairs=286
# corpus=286 match=166 fail-as-fail=111 mismatch=0 crash=0 unimplemented=4
# allowlisted=4 corpus-fail=0 skipped=1` at 55ea4ceba on dev-compute-6. The
# first run of the branch (990adea30, before the pathExists trailing-slash
# fix recorded above) measured match=165 mismatch=1 -- the floor is 166 and
# not 165 because that mismatch was fixed, not buried.
LANG_DIFF_MIN_MATCH=166

# -- rust-incremental-gate.sh ----------------------------------------------
# Arm A prints `skip` for a corpus source its compiler could not take. That
# is coverage the arm does not have, and it was uncounted: the arm asserted
# only that nothing FAILED, which a run that skipped everything satisfies.
# Measured 7 skips of 150 eval-okay files on dev-compute-6 at 297fc3e9b (it
# was 9 before the search-path work landed; a ratchet left at 9 would have
# hidden that improvement as readily as it hides a regression).
# Still 7, of 152 files, at 694a3faed on dev-compute-3: the corpus gained two
# and the arm took both.
RUST_INCR_MAX_SKIP=7

# -- drv-parity.sh, the nix build arm --------------------------------------
# Five cases. Three always: the fresh fixture, the multi-output
# `outputsToInstall` reduction, and a flake fixture with no inputs -- which is
# the whole `nix eval <flake>#attr` entry point, resolved and locked by cppnix
# and then evaluated out of `call-flake.nix` by the selected backend. Two more
# need a nixpkgs to point at: `nixpkgs hello` needs NIXPKGS, and
# `flake nixpkgs hello` needs NIXPKGS_FLAKE, which is a flake reference rather
# than a directory because the parity claim wants a pin. Exact, and the script
# subtracts from both numbers per absent case rather than letting one vanish
# unnoticed.
#
# min-match equals the case count on purpose. This arm has no refusals to
# tolerate: every case is a derivation the backend either builds identically
# to cpp or does not build, and "does not build" is the thing it exists to
# catch. Measured 3 of 3 on dev-compute-6 at the earlier tip against nixpkgs
# llgwlxshmy0ifvxh7f8wq53vk5x7vd13-source; the two flake cases were measured
# on a Mac (aarch64-darwin) where the flake entry point landed, and have not
# yet run on a Linux dev node.
DRV_PARITY_BUILD_CASES=5
DRV_PARITY_BUILD_MIN_MATCH=5

# The memo-hit case (ENG-12801), which has an assertion arm and a coverage
# arm and ratchets both.
#
# `match` is measured, not hoped for: on dev-compute-6 at the branch tip the
# assertion arm evaluated a 3M-element fold cold in 2.195s, had its `.drv`
# deleted, and re-evaluated warm in 0.044s with the file back at the same
# path and hash. So a memo hit does re-perform the derivation write, because
# the read set records it as the `Question::StoreText` it is. Watched failing
# by making `RecordingHost::write_derivation` forward without recording.
DRV_PARITY_MEMO_VERDICT=match

# The same assertion on the handle path, which `nix eval` and `nix build`
# share. It read `no` until ENG-12830, when `eval-cache-dir` wrote objects on
# that path and served nothing, so the assertion could only be run through
# `nix-instantiate`; the ratchet existed to make the day that changed
# unmissable, and this is that change. Now `match`: the same cold, delete the
# `.drv`, warm ladder runs on both arms.
#
# Measured on this Mac at the warm-starts branch tip, binary sha 05fc5737:
# `nix eval --raw -f` on a 3M-element fold cold in 1.84s and warm in 0.26s
# with the `.drv` deleted in between and back afterwards at the same path and
# sha 10bfe1788e4dabe5. Watched failing by pointing `machine_and_host` at
# `RealFs` instead of the session recorder, which makes the warm run serve an
# answer whose read set does not cover the walk.
#
# `nohit` is the value to look for in a regression: it means the warm run was
# not faster than the cold one, so the assertion below never ran.
DRV_PARITY_MEMO_BUILD_PATH_HITS=match

# -- search-path-gate.sh ---------------------------------------------------
# Every case is a literal in that script, so all three are exact. `produced`
# was a `>= 10` floor against an observed count half again as large: the same
# slack that let `served > 30` sit beside an observed 51.
SEARCH_PATH_PAIRS=19
SEARCH_PATH_PRODUCED=17
# Zero, and exact. `<x>` is implemented, so a refusal here is not scope, it is
# coverage leaving the gate -- and same() counts a refusal as neither match
# nor mismatch, so nothing else would notice.
SEARCH_PATH_REFUSED=0

# -- nixpkgs-frontier.sh ---------------------------------------------------
# The row list is checked in, so the count is exact.
NIXPKGS_FRONTIER_ROWS=12
# The frontier may advance and may not retreat. Without a floor, a backend
# that refused every row would report differ=0 and read as healthy.
# Measured 11 of 12 on dev-compute-3 at 15b970ef4 against nixpkgs
# llgwlxshmy0ifvxh7f8wq53vk5x7vd13-source. It was 6 the same afternoon, with
# all six refusals naming builtins.unsafeGetAttrPos; making that builtin
# answer null (4e4f6c7ff) moved the frontier in one step. It answers a real
# position as of ENG-12137; the count above is the measurement as taken.
#
# All twelve rows agree as of 6ddaacd80: `one package outPath` answers
# /nix/store/c2h2f4cw9p8i8zcfy52fd1dd6g0yhnki-hello-2.12.3 on both arms, which
# is the hello.outPath milestone (ENG-12591, ENG-12593, ENG-12607) reached.
#
# Raised from 11 in the same commit that reached it. Left at 11, the row could
# go back to refusing and this gate would still pass -- the floor is the only
# thing that notices a frontier going backwards, and a floor below the current
# state notices nothing.
#
# Remeasured against the tree flake.lock pins, which is what the gate now
# resolves by default; the numbers above were taken against the flake
# registry, which floats (ENG-12855). On aarch64-darwin against
# p5cm66j33sbpn8ni9f2hlr279sfhvgwq-source (nixos-25.11.6495.e764fc9a4058),
# the pinned tree scored 6 of 12 before interpolated path literals landed and
# 12 of 12 after, with all six refusals naming `path interpolation` from one
# construct in pkgs/development/interpreters/python/cpython/default.nix:404
# (ENG-12852). So the value is unchanged at 12 and now means something stable.
NIXPKGS_FRONTIER_MIN_AGREE=12
# Per row, per arm. Both arms hitting this used to score AGREE, because two
# killed processes have equal exit codes and two empty stdouts.
NIXPKGS_FRONTIER_ROW_TIMEOUT=180

# -- sigterm-gate.sh -------------------------------------------------------
# Seconds between SIGTERM and the process dying. Not total elapsed: the old
# check compared total against 15s with the signal sent at 5s, so ten seconds
# of ignoring the signal passed. The interrupt check runs every 2048 poll
# iterations, so the real number is a fraction of a second and the rest is
# unwind and print time on a loaded box.
# Measured 0.023-0.025s over three consecutive runs on dev-compute-3. Set at
# 2s: 80x headroom for a loaded box and for process unwind, and still 600x
# tighter than the 15s this replaced -- which, being compared against total
# elapsed with the signal at 5s, actually permitted a 10s reaction.
SIGTERM_MAX_KILL_DELAY=2

# -- builtins-table-gate.sh ------------------------------------------------
# Rows, and how many must agree. EXACT on both: every row is a literal in that
# file, so the count is deterministic, and `agree` must equal `rows` because
# the gate's whole claim is that the two backends advertise the same set. A
# floor here would let a row start differing without failing anything, which
# is precisely the divergence ENG-12717 was.
BUILTINS_TABLE_ROWS=18

# -- flake-inputs-parity.sh ------------------------------------------------
# Eight fixtures times five attributes. Both the fixture list and the
# attribute list are literals in that file, so the count is exact and a change
# to either must move this number in the same commit.
FLAKE_INPUTS_ROWS=40
# Measured 40 of 40 on this Mac (aarch64-darwin). Exact rather than a floor:
# every fixture is local and deterministic, so a row that stops matching is a
# regression and not weather.
FLAKE_INPUTS_MIN_MATCH=40
# One drv row per fixture, and the rust arm must have WRITTEN the `.drv` for
# every one of them. Exact and not a floor: a run where a drv row turned into
# a refusal would still clear a match floor while proving nothing about store
# paths, which is the one thing this gate may not do.
FLAKE_INPUTS_DRV_ROWS=8
# How many fixtures leave at least one lock node without an override, and so
# send it through `fetchTreeFinal` in the VM. Seven of eight; `relpath` is the
# exception and the reason is in the gate beside the number. Exact, because
# this is the count that separates a run measuring the tree fetcher from one
# measuring the override path twice, and a floor would let it decay to one.
FLAKE_INPUTS_FETCHER_FIXTURES=7
# The getFlake arm of flake-inputs-parity.sh (ENG-12995). Same eight fixtures
# and same five attributes as the command-line arm, so the row count is
# FLAKE_INPUTS_ROWS and is asserted equal rather than given its own number: a
# getFlake arm covering fewer fixtures than the arm it is compared against is
# an oracle that covers less than it claims.
FLAKE_GETFLAKE_MIN_MATCH=40

# -- rust-driver-parity.sh -------------------------------------------------
# The three numbers below were NOT measured at GATE_RATCHETS_MEASURED_AT.
# rust-driver-parity.sh and the crate it gates did not exist at that revision,
# so they carry their own stamp; the shared one still describes every ratchet
# above it.
RUST_DRIVER_PARITY_MEASURED_AT=2a9a2eedc
RUST_DRIVER_PARITY_MEASURED_ON=hydra-darwin
# Exact, and it must move in the same commit as the CASES array. A case list
# that shrank by accident otherwise reads as a clean pass.
RUST_DRIVER_PARITY_CASES=21
# Measured 21 of 21 on this Mac (aarch64-darwin). A floor and not an exact
# count only because a future case may legitimately land in a gap; today
# there is no slack, which is the point -- a floor set below what was
# measured is a floor that lets the first regression through silently.
RUST_DRIVER_PARITY_MIN_MATCH=21
# Zero, and deliberately not "a few for headroom". The ceiling is what stops
# `mismatch == 0` being achievable by refusing everything, and headroom is
# exactly the room a regression hides in. Every case in the corpus is one the
# driver can do today; the things it cannot do -- fetchers, flake locking,
# IFD, NAR ingestion -- are refused by name and are not in this corpus,
# because a gate whose cases are known refusals measures nothing.
RUST_DRIVER_PARITY_MAX_UNIMPLEMENTED=0
