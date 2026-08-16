# Mounting a revision as a filesystem

`jj fs mount` makes a revision's tree appear as a real directory, so any tool
that reads files can read a revision without a working copy and without a
checkout. It is read-only.

```shell
mkdir /tmp/rev
jj fs mount -r main /tmp/rev
```

The mount lives as long as the command does. Press Ctrl-C to unmount.

## The transport differs per platform, and the reason is macOS

There is one backend-agnostic core and two adapters over it. Which adapter runs
is the difference between an unprivileged command and one that needs a kernel
extension.

* **Linux uses FUSE.** One kernel hop, no RPC framing, and no privileges: the
  kernel's FUSE driver is what a normal user already has.
* **macOS uses NFSv3 over loopback.** macOS has no in-kernel FUSE. The
  third-party answer, macFUSE, is a kernel extension that requires enabling
  third-party kexts, which is a security posture jj will not ask a user to
  adopt. So `jj fs mount` runs an NFSv3 server bound to 127.0.0.1 inside its own
  process and asks the macOS kernel's own NFS client to mount it. This is the
  route Meta's EdenFS takes for the same reason.

Unlike EdenFS, this needs no privileged helper. Verified on macOS 27.0 (Darwin
27.0.0):

```
$ /sbin/mount_nfs -o nolocks,locallocks,vers=3,tcp,port=12055,mountport=12055,soft,timeo=50,retrans=2,ro,noresvport 127.0.0.1:/ /tmp/mnt
$ mount | grep /tmp/mnt
127.0.0.1:/ on /private/tmp/mnt (nfs, nodev, nosuid, read-only, mounted by andrewgazelka)
```

Two of those options are load bearing rather than decorative. `noresvport` is
what keeps this out of `sudo`: `mount_nfs(8)` states that a reserved source port
requires root, and nothing in NFSv3 needs one. `nolocks` keeps the client off the
NSM/statd registration path over `::1`, which is what makes other loopback NFS
servers fail on recent macOS with `EADDRNOTAVAIL` after an otherwise successful
handshake.

Pass `--transport` to override the default. On Linux, `--transport nfs` works but
needs root, because Linux, unlike macOS, requires privileges to mount NFS. That
asymmetry is why the defaults are what they are.

FSKit, Apple's kext-free filesystem API, is not used. Rust bindings exist
(`objc2-fs-kit`, `fskit-rs`), so the language is not the obstacle. The obstacle is
packaging: an FSKit module must ship as a signed `.appex` inside a signed `.app`
carrying the `com.apple.developer.fskit.fsmodule` entitlement, plus a one-time
user enable under System Settings. A CLI cannot embed an appex. Revisit if jj
ever ships a signed Mac app.

## What a conflicted file looks like

A file that is conflicted in the revision appears as a **regular file whose
contents are the conflict markers `jj` would have written into a working copy**,
using the marker style from your config. The point is that reading the mount
gives you the same bytes as checking the revision out, so nothing new has to be
learned to interpret it.

The alternatives, and what each gives up:

* **Refuse the path with `EIO`.** Safer against a tool ingesting marker text as
  if it were source, but it makes the mount unusable on any conflicted revision,
  which is a routine jj state, and it breaks every recursive tool in the
  directory rather than at the one file.
* **Expose a directory of sides,** `file/side1`, `file/base`, and so on. Richer,
  but it changes the file's type, so `ls -l` and every tree walker sees a
  directory where the revision says file, and it has no precedent elsewhere in
  jj.
* **Pick one side.** Silently wrong.

A conflict whose sides are not all files, a file against a symlink for instance,
has no marker representation at all. Those are served as a regular file holding
the same human-readable summary `jj file show` prints for them.

Two smaller decisions in the same spirit: a Git submodule appears as an empty
directory, matching what a Git checkout of an uncloned submodule looks like and
what jj's own working copy does with them; and a tree entry whose name no
filesystem can represent, `.` and `..` being the only real cases, is omitted from
its directory listing with a warning, because there is nowhere to put it.

## Sizes are exact, and cheap for ordinary files

`stat` on a file in the mount reports the true byte count, always. Never an
estimate, never zero-pending-a-fetch. That is a correctness requirement rather
than a quality bar: Nix writes a file into its store using the size from `stat`
and only then reads the content, because the NAR format puts a file's length
before its bytes. A filesystem that under-reports therefore makes Nix store
truncated content, addressed under the hash of bytes that never existed, with no
error anywhere. That is [NixOS/nix#10667], open since 2024 and triaged as hard to
fix, so the filesystem is the side that has to be right.

For an ordinary file the size comes from the store's metadata and nothing is
read. `Backend::file_size` defaults to streaming and counting, which is correct
for any backend, and the Git backend overrides it to read the object header: a
loose object carries its length in the first bytes of its zlib stream and a
packed one in the pack entry header, so neither is inflated.

Measured over 3000 files totaling 98 MB, cold stat sweep, each run against a
freshly created loopback NFS mount so nothing is cached anywhere:

```
without the size API      1.57s      1.51s
with the size API         0.31s      0.32s
```

About five times faster, reporting byte-identical sizes, and it pulls nothing
into the content cache instead of pulling all 98 MB through it.

Two paths still have to build the content before they can size it, because for
them no stored size exists to ask for: a conflicted file, which is not bytes at
all until its sides are merged into marker text, and a symlink, whose target is
not a stored blob. Those are the minority of entries in any tree.

Contents are cached against a byte budget (`--content-cache-bytes`, 256 MiB by
default) so a repeated read is paid once. A file larger than the whole budget is
served uncached and re-read on each access; `cached_content_bytes()` reports what
is held, which is how the tests tell "answered from metadata" apart from "read
the file".

Directory listings are cached, and because the tree is immutable the FUSE adapter
tells the kernel it may cache attributes and lookups for a day, which removes
almost all repeat traffic.

## Partial clones: lazy from the remote, not only from the object store

The mount was already lazy against the local object store: mounting reads the
root trees and nothing else, directories are read on first listing, and a
file's bytes are read on first open. A partial clone extends that laziness over
the network. In a `git clone --filter=blob:none` repository the object store
holds commits and trees but no file contents, and Git's promisor machinery
fetches a blob from the remote the first time something asks for it.

jj reads objects through gix, which has no promisor support: without help, a
missing blob surfaces as `ObjectNotFound`, which the mount serves as `EIO`. So
the Git backend backfills: when `find_object` or `find_header` misses **and**
the repository names a promisor remote (`remote.<name>.promisor = true`, the
marker `git clone --filter` writes), the backend runs `git cat-file -e <oid>`
against the backing repository, which triggers Git's own lazy fetch, and
retries the lookup once. Delegating to the CLI is the point rather than a
shortcut: which remote to ask, what filter to send, and whether lazy fetching
is allowed at all (`GIT_NO_LAZY_FETCH`) stay Git's decisions, so the repository
behaves the same read through jj as read through git. In a repository with no
promisor remote the miss path is untouched: no subprocess, same hard error as
always.

Measured against github.com (jj-vcs/jj, 704 files, 12.4 MB checkout, 173 MB
full-clone `.git`): the `blob:none` clone is 11.8 MB, the mount appears in
26 ms, each first read of a never-fetched file costs one fetch round trip
(210-260 ms observed), a re-read costs 2 ms from the content cache, and seven
files read through the mount moved 188 kB over the wire. Nothing else was
fetched, which is the definition being claimed.

Two costs to know about. In a partial clone a file's size lives with its
bytes, so the `Backend::file_size` fast path described below has nothing local
to read for a never-fetched file and the backfill fetches the whole object:
`stat` over cold files is as expensive as reading them, and a tool that stats
the whole tree (`du`, a stat sweep) will fetch the whole tree. And each missing
object is fetched by its own subprocess, one round trip per file; a workload
that is about to read a subtree wholesale is better served by one
`git archive <rev> -- <subtree> >/dev/null` in the backing repository first,
which batches the same fetches into one pack.

`lib/tests/test_git_backend.rs` drives the backfill against a local
`file://` promisor remote (`test_partial_clone_backfills_missing_blobs_on_read`)
and pins the no-promisor miss path
(`test_missing_object_without_promisor_is_still_not_found`).

## What it costs against local disk, and the 165x worry it answers

The standing fear about a mount was ENG-5478: nixpkgs as a `path:` flakeref on a
virtiofs mount made `nix eval` take 2m45s per run against 1.0s local, with kernel
stacks dominated by fuse operations, and the conclusion recorded there was that
cost scales with filesystem latency. That is roughly **165x**, and it is the reason
an unnecessary tree walk on a mount has been treated as costing about 100x.

Measured with `vfs/bench/tree-walk.sh`, 3000 files totaling 98,299,936 bytes:

```
FUSE, Linux                       mount    local disk
  find -type f, names only          4 ms        18 ms
  stat sweep, one stat per file     64 ms         7 ms
  read every file (98 MB)          211 ms        64 ms

NFS, macOS                        mount    local disk
  find -type f, names only         21 ms        38 ms
  stat sweep, one stat per file    20 ms        32 ms
  read every file (98 MB)        2312 ms       135 ms
```

**FUSE on Linux answers the worry: a full read is about 3x, not 165x**, roughly
470 MB/s. Nix NAR-hashes, so it reads content, which makes the full-read row the
one that governs a Nix evaluation over the mount. A name-only walk is faster than
local disk, because the tree is already in memory and there are no inode reads.

**NFS on macOS costs per file opened, not per byte read**, and calling it "17x" or
"42 MB/s" the way an earlier version of this page did was misleading. The same
98 MB through the same mount, two shapes:

```
3000 files of ~33 KB      2683 ms    ~37 MB/s
1 file of 98 MB             18 ms  ~5461 MB/s
```

One large file reads *faster than local disk*, because it is served from the
in-memory content cache. So bulk throughput is not a problem at all; the cost is
about 0.85 ms of added latency per file opened, against 0.04 ms locally. A tree of
many small files pays it 3000 times, which is what produced the alarming-looking
bandwidth figure.

That distinction matters for deciding whether the mount is usable: reading a few
large files through it is free, and walking a source tree of thousands of small
ones is where it hurts. It also points any future work at round trips rather than
at transfer sizes.

Two things that do not help, measured rather than assumed, because hf-mount sets
both and it is the obvious thing to try: `rsize=1048576` makes **no difference**
(2188 ms either way), which follows once you know the average file is 33 KB and
therefore already one read. And `actimeo=1` makes it **worse** (2930 ms).

Two things about the middle row. It is a **post-size-API** number: before
`Backend::file_size`, a stat sweep read every file it touched, so that row would be
roughly as slow as the read row rather than a fraction of it. The two results
compose, and neither is the whole story. And on the NFS side the client's attribute
cache means the row measures caching as much as it measures us.

**NFS on Linux is unmeasured.** The benchmark refuses on the one Linux host
available here, because it is not configured as an NFS client:

```
Error: mount failed: mount: /tmp/.../mnt: fsconfig() failed:
NFS: mount program didn't pass remote address.
```

The NixOS module sets `boot.supportedFilesystems.nfs`, which is what makes it work
in the VM test, and reconfiguring a shared build host for a benchmark is not worth
it. Expect it between the two rows above and closer to the macOS one, since the
cost is RPC round trips rather than the platform.

The benchmark checks the tree it built, twice, on disk and again through the mount,
and refuses rather than reporting if either count is wrong. That is not ceremony:
the first version of this measurement ran in a shell without `python3`, built
nothing, and returned `0.00s` for every row. A benchmark over an empty input
returns plausible numbers.

## Testing

Two levels, and one honest gap.

**Unit level, any platform, no privileges.** The tree walk plus the NFSv3 server
driven end to end by an in-process Rust NFS client over a loopback socket. That
covers real XDR encoding, RPC framing, the MOUNT handshake and every procedure
implemented, without a kernel mount:

```shell
cargo nextest run -p jj-vfs
```

**Checking the Windows build without a Windows machine.** `jj fs mount` is
`cfg(unix)` below the command level, so the `#[cfg(not(unix))]` arm is compiled
only for a target nobody here develops on. It is still checkable locally, in
about thirty seconds:

```shell
nix shell nixpkgs#pkgsCross.mingwW64.stdenv.cc --command bash -c '
  export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
  export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc
  cargo check -p jj-cli --all-targets --target x86_64-pc-windows-gnu'
```

Two constraints, both of which cost an attempt to discover:

* **`x86_64-pc-windows-gnu`, not `-msvc`.** `libmimalloc-sys` runs a build script
  that needs a C compiler for the target, and msvc's is not available off
  Windows: the check dies with `VCINSTALLDIR = None`. That is true even for a
  single-package check, so there is no way around it. mingw is a C compiler that
  does exist here. The cfg logic is identical either way, because `unix` is false
  for both.
* **Run it on macOS, not on the Linux builder.** nixpkgs' `rustc` ships no
  Windows `std`, so there the check fails with `can't find crate for 'std'`. The
  Mac's rustup has it after `rustup target add x86_64-pc-windows-gnu`.

The recipe was confirmed to actually cover the non-unix arm by planting a
deliberate error in it and watching the check fail.

**Integration level, real mounts, Linux only.** A NixOS VM test mounts a revision
both ways and checks it through actual kernel clients: listing, `find` over the
whole tree, `cat`, a sha256 compared against the same content read out of the
repository with `jj file show`, a 300 kB file so that multi-read offsets are
exercised, a symlink both read and followed, the executable bit including
actually running the script, `pwd -P` from a subdirectory so that `getcwd`
resolves `..` through the filesystem rather than through the shell's idea of the
cwd, writes refused, and a clean unmount on SIGTERM with nothing left in the
mount table.

```shell
nix build .#vfs-mount-test
```

It is a package rather than a flake check on purpose. `nix flake check` runs on
GitHub hosted runners and those do not provide KVM; the build does not degrade to
emulation there, it refuses outright:

```
error: failed to build attribute 'checks.aarch64-linux.vfs-mount'
       Reason: missing system features
       Required features: {kvm, nixos-test}
       Available features: {benchmark, big-parallel, nixos-test, uid-range}
```

`nix flake check` only builds `checks`, so as a package it is evaluated
everywhere and built only where someone asks for it. Run it on a host with KVM.

**The macOS mount has no automated coverage.** This is a gap, not an oversight,
and it will not close on its own:

* `nixosTest` needs a Linux kernel and KVM, so it cannot run on macOS at all,
  and GitHub hosted Linux runners do not provide KVM either.
* The macOS Nix build sandbox will not let a derivation mount anything.
* CI here is Linux-only self-hosted runners with no Mac in the fleet.

So the macOS path is verified by hand, and the recipe below is what was actually
run. Treat it as unverified by CI, because it is.

```shell
mkdir -p /tmp/jj-mnt
jj fs mount -r @ --transport nfs /tmp/jj-mnt &
until mount | grep -q /tmp/jj-mnt; do sleep 0.5; done

mount | grep jj-mnt
ls -la /tmp/jj-mnt
cat /tmp/jj-mnt/some-file
shasum -a 256 < /tmp/jj-mnt/some-file      # compare against `jj file show`
readlink /tmp/jj-mnt/some-symlink
test -x /tmp/jj-mnt/some-script && echo executable
( cd /tmp/jj-mnt/some-dir && cd .. && pwd -P )   # must print /tmp/jj-mnt
echo nope > /tmp/jj-mnt/newfile            # must fail: Read-only file system

kill -TERM %1
until ! mount | grep -q /tmp/jj-mnt; do sleep 0.5; done
```

## On macOS, `test -x` says yes for every file

`stat` reports the right mode everywhere, so `ls -l` and anything that reads the
mode sees the truth. `access(2)`, which is what `test -x` and `[ -x ]` use, is a
different story and the answer differs by client:

* **FUSE on Linux: correct.** The mount carries `default_permissions`, so the
  kernel does the check itself against the mode we report.
* **NFS on Linux: correct.** The client checks locally against the attributes it
  has cached, and the VM test asserts this.
* **NFS on macOS: wrong.** macOS trusts the NFSv3 ACCESS reply, and the server
  library we use (`nfs3_server` 0.11.0) answers ACCESS without consulting the
  mode: on a read-only export it grants read, lookup and execute for everything.
  So `test -x` returns true for a mode 0444 file. Observed on macOS 27.0 as an
  unprivileged user against a file we own with mode 0444.

The proper fix belongs in `nfs3_server`, whose ACCESS handler should mask the
requested bits against the mode from the attributes it already fetches. Doing it
downstream would mean carrying a fork as a git dependency, which `deny.toml`
disallows (`allow-git = []`), so that is a policy decision rather than something to
slip in here. Filed as ENG-11614.

Nothing in CI can catch a regression here, for the same reason the macOS mount has
no coverage at all: there is no Mac in the fleet.

## The journal, which v0 does not have

There is a seam for recording every change, and nothing writes to it, because a
read-only mount has no changes to record. It is in the tree now because the shape
decides whether a future write path can answer "what did this tree look like at
14:03" cheaply, and that has to be chosen before two transports each grow their
own answer.

The reasoning is worth stating even though the code is not here. jj's operation
log records every change to refs, which is good, but it learns about the working
copy by polling: each command walks the tree and diffs it, so everything between
two commands collapses into one net change. Edit a file five times between two jj
commands and jj sees one edit. A virtual filesystem is not a poller, it *is* the
write path, so the complete ordered sequence is available for free and the only
way to not have it is to throw it away.

The design it is shaped for is Google's CitC split of a workspace into a baseline
(the state at a submitted commit) and an overlay (what the user changed). The
observation that makes the two one thing rather than two: **the overlay is the
journal's current state.** Build the overlay as a replay of an append-only log
rather than as a mutable tree, and reconstructing the tree at any instant falls
out of the same structure that made snapshotting cheap, instead of needing a
second history mechanism beside it.

Three constraints are already settled, because each is cheaper to honor by
construction than to retrofit:

* **Attribution is optional, per transport.** A FUSE request carries the calling
  pid and uid, so on Linux an entry can say which process wrote which file in what
  order. NFSv3 carries no such field, so on macOS the journal records what changed
  and when but not who. EdenFS hit the same wall and fell back to sampling `lsof`
  and `fs_usage`, which is not attribution. The field is optional in the data
  rather than filled with a guess, so the absence is visible.
* **Content is recorded by reference and content-addressed**, so a journal of a
  `cargo build` is a list of ids rather than a second copy of `target/`.
* **The journal never goes through the mount it records.** Over NFS neither client
  sends COMMIT on `fsync`, so a journal written through the mount can lose its
  tail while looking complete, which is worse than no journal. The NixOS module
  refuses a `journalDirectory` inside the mount point at eval time.

Two things are deliberately open: retention and exclusion, since recording every
write captures editor swap files, build output and a secret typed into a file and
deleted a minute later, and "exclude by gitignore" is not sufficient because the
point is to record what git would not see; and coalescing, which is the knob that
destroys the property the journal exists for, so any coalescing needs a stated
rule rather than a heuristic.

## Case sensitivity

The mount is case-sensitive: `Foo` and `foo` are two entries with two inodes and
two contents. That is not cosmetic. Nix's `use-case-hack` defaults on for darwin
and appends a `~nix~case~hack~` suffix when it sees names colliding
case-insensitively, so a case-folding mount would bake those mangled names into
the NAR and make store paths computed on a Mac diverge from Linux for identical
content, surfacing as a cache miss nobody can explain.

Tested two ways. A unit test builds a tree containing `Foo`, `foo`, `dir/Bar` and
`dir/bar` through the store and asserts four distinct entries with the right
contents; it goes through the store rather than a working copy precisely because
APFS is case-insensitive and the two files cannot both exist on the machine that
most needs the guarantee. The NixOS VM test then creates both in the working copy
on case-sensitive ext4 and asserts both are visible with distinct contents through
each transport.

**macOS is case-sensitive too, measured rather than assumed.** There was prior
evidence that loopback NFS mounts on macOS are case-insensitive with
first-casing-wins, which would have been serious: it is exactly the condition
`use-case-hack` exists for. It does not hold for this server. Verified on macOS
27.0 by building the colliding tree on Linux, copying the jj store to a Mac (only
`.jj` and `.git`, since extracting the working copy would have collapsed the pair
before the mount ever ran), and mounting it:

```
$ ls -A /tmp/mnt
foo
Foo
sub
$ cat /tmp/mnt/Foo
UPPER content
$ cat /tmp/mnt/foo
lower content
$ ls -i /tmp/mnt/Foo /tmp/mnt/foo
2 /tmp/mnt/Foo
3 /tmp/mnt/foo
$ cat /tmp/mnt/FOO
cat: /tmp/mnt/FOO: No such file or directory
```

Both names present, distinct contents, distinct inodes, and a casing that does not
exist is refused rather than folded onto one that does. That last line is the one
that settles it: a case-insensitive mount would have served `Foo` for `FOO`.

This is not automated. It needs a Linux host to build the tree and a Mac to mount
it, which no single CI job has, so it is a manual check like the rest of the macOS
story. The recipe above is the whole of it.

## A writable scratch layer, and the one thing it still cannot do

**This is a stepping stone and its semantics will change.** The flag is
`--scratch` rather than `--writable` on purpose: a writable mount should mean a
mount backed by a real `jj` workspace, where a write to a tracked path becomes a
commit that `jj log` shows and `jj undo` reaches. That is the intended end state
and the name is reserved for it. What a write to a tracked path means here is
not what it will mean then, so nothing should be built on the current
behavior.

`jj fs mount --scratch` accepts writes. They land in a real directory on the
host, reads resolve that directory first and the revision second, and nothing is
ever written to the object store. It exists because build tooling cannot run
against a read-only mount: the reported failure was

    ~/mnt/repo/packages/web $ bun install
    EROFS: Read-only file system: could not create the "node_modules" directory (mkdir)

The property the implementation defends is that **nothing done through the
mount can cost you anything the revision holds.** The lower layer is immutable
and content-addressed and is never written to, so deleting the scratch
directory restores the revision exactly, whatever happened in between.

A tracked file or symlink can be shadowed and it can be deleted. Writing to one
copies it into the scratch layer and the name then resolves to the copy;
deleting one records a whiteout, and the name stops resolving until something
creates it again. A tracked directory can only be added to: removing one, and
renaming one away, are refused with `EROFS`.

Directories are the exception because hiding one means hiding a subtree, which
needs opaque directories and readdir subtraction below the whiteout. Hiding a
file needs one name in one set, tested at the two points that turn a name into
an entry. So `rm -rf src` still fails, but at the `rmdir` rather than at the
first file, and `rm -rf node_modules` works because `node_modules` is not in
the revision at all.

### A whiteout belongs to a revision, not to a path

This is the question the first cut of the scratch layer left open, and the
answer is that a whiteout is scoped to the revision it was made against.

Deleting `bun.lock` says something about the file *this* revision has at that
name. It says nothing about whatever a different revision has there. So the
whiteout log carries the tree it belongs to on its first line, and a scratch
layer remounted at a different revision starts with no whiteouts and every
tracked name back. The scratch *files* are unaffected and still persist across
the remount, which is the whole reason the layer outlives one mount.

That is the safe direction to be wrong in: changing revision can only make the
mount show more names, never fewer. Carrying whiteouts across revisions hides
files at a revision where nobody asked for them to be hidden, and there is no
way for a user to tell that has happened short of comparing against `jj file
list`.

The log is `.jj-overlay-whiteouts` in the scratch directory, beside the lock. It
is append-only, one `- path` or `+ path` record per operation, and rewritten
when the superseded records outnumber the live ones, because a
watch-and-rebuild loop deletes and recreates the same lockfile once per build.
It is not fsynced: a crash can lose the last few whiteouts and resurrect names
the caller deleted, which is the same direction of error as remounting at a new
revision and much cheaper than an fsync on the unlink path.

Three names in the scratch directory are the layer's own — the lock, the log,
and the log's rewrite temporary. None of them is listed, none resolves, and
creating or deleting one through the mount is refused. Before this they were
merely unlisted, so `cat .jj-overlay-lock` through a mount served the lock file
and `rm` of it would have broken the mount.

### `bun install` completes

Against the ix workspace on macOS 27 the install used to run to completion and
then die at the last step:

    bun install v1.3.13
    Resolving dependencies
    Resolved, downloaded and extracted [62]
    EROFS: Failed to replace old lockfile with new lockfile on disk

With `JJ_LOG=jj_vfs=warn` the server named the single refusal in the entire run:

    WARN jj_vfs::nfs: refused a write err=cannot rename away bun.lock, which is
    in the revision

`bun.lock` is tracked, and bun replaces a lockfile by renaming the old one
aside before moving the new one into place. Renaming *onto* a tracked path was
already supported, since that is the write-to-temp-then-rename idiom almost
every careful writer uses. Renaming one *away* is what a whiteout adds, and
`test_the_lockfile_replacement_that_bun_actually_does` drives that exact
sequence: write a temporary, rename the tracked lockfile aside, rename the
temporary onto it, delete the one that was set aside.

A tracked name renamed away has to be copied up first, because `rename(2)` moves
a real file and a tracked name that has never been written to does not have one
yet. Getting that wrong renames nothing and reports success, which is why the
test reads the destination's content rather than only its name.

### Where the scratch layer lives

Under the platform cache directory, keyed on the mountpoint:

    ~/.cache/jj/fs-overlay/-private-tmp-jjfs-overlay-mnt-b262eaffd92c5742/

Keyed on the mountpoint and not on the revision, deliberately. Keying on the
revision would re-run `bun install` on every `jj new`, and repopulating a
`node_modules` is the cost the persistence exists to avoid. The name keeps the
mountpoint path readable rather than reducing it to a hash, because the one thing
a user needs to do with this directory is find the right one and delete it; the
FNV-1a suffix is spelled out in the source rather than taken from a crate,
because it is baked into a directory name and `DefaultHasher` does not promise to
mean the same thing between releases.

It is deliberately not inside `.jj/`, which jj's own snapshotting watches, and
not inside the mountpoint. Pass `--scratch DIR` to choose the directory yourself.

The directory is held under an exclusive `flock` for the life of the server, and
a second mount of the same layer is refused by name rather than made to wait.
That is not hypothetical: a `jj fs mount` can outlive its own mountpoint (see
ENG-11688), so remounting the same path is exactly how two servers end up sharing
one scratch directory, and without the lock they would be two writers with
nothing between them.

### What it costs, and why

The whole difference is per-operation cost, and the model closes to within a
percent:

| | wall clock | operations | per operation |
|---|---|---|---|
| APFS | 3.75s | ~5.4M | 0.70 us |
| loopback NFS mount | 120.9s | 5,394,137 | 22.4 us |

Per-operation ratio 32.0x, wall clock ratio 32.2x. The same 5.4 million
operations happen on both filesystems, and the entire difference is what each
one costs. There is no missing factor.

That took three failed attempts to establish, and the failures are the evidence.
Discarding the AppleDouble sidecars, lengthening the NFS attribute cache to ten
minutes, and filling in READDIRPLUS attributes each attack operation *count*,
and each moved it by under 2%. The count is not ours to reduce: `bun` stats
paths it constructs from the lockfile rather than paths it discovers by listing,
which is why READDIRPLUS attributes in particular are never consulted. 22,669
listings against 3,397,989 stats is not a listing-then-stat pattern.

The transport is not misconfigured either. A bare loopback TCP request and
response on the same machine, `TCP_NODELAY` on, 128-byte messages, measures
18.52 us. Our transport costs 16.8 us of the 22.4, so it is already at the floor
for a loopback socket; `nfs3_server` sets `TCP_NODELAY` itself and dispatches
each RPC on its own task, so neither Nagle nor serialization is in play. The
only way past this is not to use a socket.

Which bounds what a different transport can buy. A userspace filesystem pays a
process boundary on every call, so its floor is a context switch pair, on the
order of 3 us against APFS's 0.70. APFS is fast here because those 3.4 million
stats are in-kernel cache hits that never reach userspace at all. The question
worth asking of FSKit is therefore not what an upcall costs but whether it lets
the kernel answer without one.

**A practical note for anyone deciding whether to build inside a mount.** This
workload issues about 140 filesystem operations per file, and every one of them
costs roughly 32x what it costs on local disk. Tools vary enormously in how
operation-hungry they are, and the ones that stat aggressively will feel this
far more than the ones that do not. The mount is not uniformly slower; it is
slower in proportion to how much a tool talks to the filesystem.

### The AppleDouble surprise

The same 630-package `bun install` on the ix workspace:

| | wall clock | entries created |
|---|---|---|
| APFS | 4.06s | 38,760 |
| writable NFS mount | 114s, 125s | 38,517 real, plus 39,003 `._*` sidecars |

About 30x, and roughly half the file operations were not the caller's. macOS
stamps `com.apple.provenance` on these files, confirmed with `xattr -l` against
the mount, and NFSv3 has no extended attributes, so the client materializes each
one as a 4.1 KB AppleDouble `._name` file beside the real one. 39,003 of them,
about 160 MB, and they appear in listings through the mount.

`mount_nfs` has a `namedattr` option that would avoid this and it is NFSv4-only,
so there is no flag that fixes it on the transport we use. The server therefore
hides and discards `._*` names as a deliberate macOS accommodation: a create is
accepted, a write is counted and thrown away, and the name never appears in a
listing. What is being dropped is a transport artifact carrying an xattr with no
representation in a jj tree, not anything a caller put there on purpose. The
same install on APFS produces zero sidecars, which is the check that they belong
to the transport rather than to bun.

If we ever move to a transport with native extended attributes, this
accommodation should be deleted rather than carried.

30x is worth paying once for a layer that then persists across every remount. It
is not worth paying per build, which is the argument for the persistence design
rather than an ephemeral scratch directory.

### Writable mounts change three things elsewhere

* Mode bits. A read-only mount reports `0o444` and `0o555`, which is accurate. A
  writable one has to report `0o644` and `0o755` or the kernel refuses every
  `mkdir` and every open-for-write before the server is asked. Honest for files,
  since copy-up means a tracked file really can be written; not honest for
  directories, because POSIX has no mode bit for "you may add a name here but not
  remove one", so a caller that checks the mode before unlinking is told yes and
  then gets `EROFS`.
* `mtime` is per entry rather than per mount. Every incremental build system
  decides what to rebuild by comparing timestamps, and reporting the commit's
  timestamp for a file written thirty seconds ago makes them skip work.
* NFS readdir cookies are positions, not fileids, and `NFS3ERR_BAD_COOKIE` is
  never returned. A directory with a scratch layer is mutable, so the entry a
  fileid cookie named can be gone by the next page. nfs3_server reached the same
  conclusion about its own cookieverf check and disabled it, recording that
  BAD_COOKIE makes the macOS client fail a listing with "no such file or
  directory". The cost is that a directory modified between two pages of one
  listing can skip or repeat an entry, which needs a directory large enough to
  paginate and a write landing inside the same listing.

### The NFS timeout is longer when writable

A read-only mount can afford `timeo=50`, five seconds: losing a read to a wedged
server costs a retry. Losing a write costs the bytes, because `soft` returns
`EIO` and a package manager mid-install has no idea it happened, so a writable
mount uses `timeo=600`. `hard` would remove the loss entirely and is not used,
because a hard mount whose server has exited leaves every process touching it in
uninterruptible sleep, which is the failure this command already goes out of its
way to avoid.

## `--writable`: a working copy, not a commit per write

Design only. Nothing below is built, and the numbers cited are from the
`--scratch` measurements above rather than from a prototype.

`--writable` is the reserved name for a mount backed by a real `jj` workspace,
where writing a file changes what `jj log` and `jj undo` see. The obvious
reading of that is a commit per write. One number rules it out.

### 290,879 writes

That is the write count from a single `bun install` through the mount, from the
same run as the operation table above. A commit per write is 290,879 commits.
jj records an operation when the working copy changes, so the operation log
grows by the same order, and `jj undo` steps back one `write(2)` at a time.

No amount of batching rescues the shape. Any batch boundary is a guess about
when a tool has finished writing, and a wrong guess produces a commit that
splits a file's contents across two revisions.

The alternative is that the mount becomes the workspace's working copy. Writes
accumulate in the server, and jj turns them into a tree when a command asks.
The commit boundary is then the one jj already has, and there is nothing new to
invent.

### What a commit per write would buy, and what it costs

This is the open question for whoever is deciding the shape, so here is the
trade with the numbers attached rather than an assertion.

A commit per write buys one thing: every intermediate state is in the object
store the instant it exists, so nothing is lost even if the machine dies
between commands.

It costs 290,879 commits per `bun install`, an operation log of the same order,
and a `jj undo` that steps back one `write(2)` at a time.

The cost that matters more is that almost none of those commits is a state
anyone would want to return to. A writer emits a file in chunks, so a commit
taken after each `write(2)` holds a half-written file. The recoverable history
would be mostly torn files that never existed as complete objects on any
filesystem. Volume alone could be argued about; the content cannot.

Continuous snapshot state buys the other half: the answer to "what has changed"
is correct at every instant and costs writes-since-snapshot to produce, `jj
log` shows one working-copy commit as it does now, and `jj undo` steps at
command granularity.

What it gives up is durability in the window. Between a write and the next fold
into `@`, the bytes are in the server's own storage rather than the object
store, so a crash in that window depends on the server's persistence rather
than on jj's.

There is a middle that appears to get both, and it is the recommendation here.
Fold writes into `@` on the same flush rule the state file already needs: on
write-idle, on an NFS `COMMIT`, and on a hard cap. jj amends the working-copy
commit rather than adding to it, so `jj log` still shows one commit and the
count is one amend per flush rather than one per write. Durability then lands
within about a second, in the object store, and no torn file is ever the head
of anything.

The question that needs an answer before step 2 is built: is durability within
about a second of a write enough, or is there a use that needs each individual
write recoverable? If the second, the cost above is the price and it should be
paid deliberately.

### Where it plugs in

jj has a pluggable working copy, and the seam is complete enough to use as-is:

- `lib/src/working_copy.rs:53` `trait WorkingCopy`, `:110` `trait
  LockedWorkingCopy`, `:86` `trait WorkingCopyFactory`
- `lib/src/workspace.rs:568` `type WorkingCopyFactories = HashMap<String, Box<dyn
  WorkingCopyFactory>>`
- the chosen implementation is a string in `.jj/working_copy/type`, written at
  `workspace.rs:160` and read back at `:632`, with the lookup at `:511`
- `cli/src/cli_util.rs` already imports `default_working_copy_factories`, so
  passing a different map is a change to one call

Upstream has considered this case. The `set_sparse_patterns` TODO in
`working_copy.rs` reads "for working copies that don't support sparse checkouts
(e.g. because they use a virtual file system so there's no reason to use
sparse)".

So `jj fs mount --writable` loads the workspace with a `jj-vfs` factory
registered and serves `@`. Serving `@` instead of a fixed revision is the whole
difference from `--scratch`.

### The tracking rule is jj's, not ours

`SnapshotOptions` at `working_copy.rs:216` already decides which files become
part of a tree. New untracked files are tested against
`start_tracking_matcher`. Files that are ignored or oversized are tested
against `force_tracking_matcher`. Files that are already tracked are always
snapshotted, which the field comment states outright.

Use that rule. Do not write a gitignored-versus-not rule beside it.

The argument is agreement. A mount with its own idea of which files count means
`jj status` run inside the mount reports a different set of files from the one
the mount is serving, and the user has no way to tell which is right. Two
definitions of "tracked" in one filesystem is a bug that presents as confusion
rather than as an error.

Adopting jj's rule also settles three cases that a gitignore rule leaves open:

A new source file, untracked and not ignored, is tracked and written through.
This is the case a gitignore rule gets wrong. Under `--scratch` such a file
goes to the disposable layer and is thrown away with it.

A file that is gitignored but tracked anyway, which both git and jj permit, is
written through, because already-tracked files are always snapshotted.

A change to ignore rules while the mount is up takes effect at the next
snapshot. `base_ignores` is passed per snapshot rather than held on the tree
state, and the comment at `working_copy.rs:219` gives the reason: "the
TreeState may be long-lived if the library is used in a long-lived process".
Already-tracked files stay tracked, because adding a path to `.gitignore` does
not untrack it.

The disposable layer survives for paths that are genuinely ignored and
untracked. A `node_modules` never enters a tree, and deleting the scratch
directory is the disposal.

### The mount is dirty; measuring the dirt is cheap

It is tempting to describe this as a mount that is never dirty. That is not
accurate and the inaccuracy will mislead someone later.

The working copy is dirty between a write and the next snapshot, exactly as a
local working copy is. What changes is the cost of finding out. The server
handled every write, so it knows the changed set already, and producing the
tree is proportional to the writes since the last snapshot rather than to the
38,517 entries in the tree. A local working copy has to walk the tree to
discover the same thing.

The property comes from the server keeping that bookkeeping, not from NFS and
not from FSKit. Any transport that routes writes through one process can do it,
and a transport that does not cannot.

### Eager or lazy: when the changed set is computed

"The same as `jj status`, but on every write" is a statement about when the
answer is recomputed, not about when a commit is written. It is now the live
decision, so it is recorded here with its cost.

Note first that both readings maintain the changed path set eagerly. The server
learns the path on every write for free, and any design that did not record it
would have to walk the tree to find out, which is the cost this whole approach
exists to avoid. The real question is whether file content is compared eagerly
too.

Option A, eager paths and content compared at snapshot. Status is available at
any instant and costs the size of the changed set. The cost is that a file
written back to its original bytes reads as modified until the next snapshot
reconciles it, because a path is in the set as soon as it is written and
nothing has yet compared the bytes. `jj status` would report a file that jj
itself would call unchanged.

Option B, eager paths and content hashed on write. Exact at every instant. The
cost is a hash per write, and the common pattern is a sequential append, which
re-hashes the whole file once per write to it. That is quadratic in file size,
against a workload measured at 290,879 writes.

Option C, lazy. Exact and cheap, and the answer is only as current as the last
command, which is the thing this section exists to change.

A is the recommendation. The false positive is narrow, it is visible only to a
reader who looks between a write and the next snapshot, and it resolves itself
rather than persisting. B pays a real and growing cost to close a window that
closes on its own.

### NFSv3 has no close, and the problem shrinks rather than disappears

A commit per write needs to know when a write sequence ends. NFSv3 has no
`close`, so the server would have to infer it, and a wrong inference is a wrong
commit.

Making the VFS the working copy removes that question and replaces it with a
smaller one. jj commands are separate processes. `LocalWorkingCopy` is rebuilt
from disk state on every `jj` invocation, so a `jj status` in another terminal
cannot see state that lives only in the running server.

Two ways to bridge that. The server can answer over IPC, or the server can
persist its tree state to `state_path` so `load_working_copy` reads it. The
second is much less machinery and matches how the scratch layer already
persists.

The state file needs a flush boundary, which is where the inference comes back.
The proposed rule: flush when no write has arrived for a few hundred
milliseconds, flush on an NFS `COMMIT`, and flush on a hard cap so a continuous
writer does not starve.

What it costs when it guesses wrong: a `jj status` that lands inside the window
reads a tree a few hundred milliseconds stale. The next flush corrects it. No
data is lost and no commit is wrong, which is a much better failure than a
mistimed commit, and is most of the argument for this design.

### Whiteouts do not come with it

`--scratch` needs whiteouts because the layer underneath is a revision that
cannot be edited, so hiding a name requires a record saying the name is gone.

`--writable` has no such layer. Underneath is `@`, a commit jj rewrites on every
snapshot, so a deleted path is a path absent from the next tree. Nothing insists
the name still exists, and there is nothing to suppress.

Carrying whiteouts into `--writable` would mean maintaining a subtraction set
against a layer that already agrees the file is gone, with two places able to
disagree about it. The whiteout log, its revision scoping, its escaping and its
compaction all become weight with nothing to hold up.

They stay in `--scratch`, which serves a fixed revision on purpose, and they are
correct there. If `--writable` ever grows a mode that serves a revision other
than `@` writably, whiteouts are the answer again and should be reused.

### The risk to watch

Three pieces of state in the scratch layer are sound only because the serving
process is the only writer and holds the `flock`: the attribute cache, the
`shadowed` map, and the whiteout log. If anything reads or writes a scratch
layer without taking the lock, all three go stale at once.

Persisting tree state for other processes to read adds a fourth piece with the
same assumption, and it is the first one another process is meant to read by
design. That inverts the assumption the other three rest on. This is the part
of the design most likely to be wrong, and the part to prototype before
trusting the estimate below.

### What is not verified

Trait definitions and the workspace loader were read. The snapshot internals in
`local_working_copy.rs` were not. Whether `check_out` for a VFS is the cheap
tree-pointer swap it appears to be is unconfirmed, and if it is, `jj new` and
`jj edit` stop writing files at all, which may be worth more than the write
path. The choice between IPC and a state file is reasoning rather than
something read from the code. No prototype exists and nothing here is measured.

### Size

Several days.

1. A `jj-vfs` `WorkingCopy`, `LockedWorkingCopy` and `WorkingCopyFactory` that
   serves `@` read-only and snapshots to an unchanged tree. About a day. Build
   this first and stop: if the seam does not hold, the rest of the estimate is
   worthless.
2. The write path. Accumulate writes, implement `snapshot()` against jj's
   tracking rules, then `check_out`, `reset` and `recover`. Two to three days.
3. Cross-process state: persisting the tree state, the flush rule, and the
   locking question above. About a day, and the riskiest.
4. Making `jj status`, `jj log` and `jj diff` agree with the mount, with tests
   that prove it. About a day.

## FSKit: what it would buy, and the one question that decides it

The measurements above say the loopback socket is the cost and that our own
handlers are nearly free. FSKit is a kernel-to-userspace interface with no RPC
layer, so it is the obvious next transport. This section records what was
established without building one, so nobody re-derives it.

### It permits kernel-side caching, which is the whole point

The naive case for FSKit is that an upcall is cheaper than a socket round trip.
That case is real but small: a userspace filesystem pays a process boundary on
every call, so its floor is a context switch pair, on the order of 3 us against
APFS's 0.70. At 5.4 million operations that is still tens of seconds.

The real case is that FSKit does not require every operation to cross. From
`FSVolumeHandlerResult.h`:

> Important: Be sure to populate all requested attributes. FSKit caches all
> populated attributes and may use them in subsequent operations, even if not
> explicitly requested.

And `FSVolumeDataCacheHandler.h` goes further, into a coherency level the module
*grants* per item:

    FSKernelCacheCoherencyTypeNoCache        all I/O goes to storage
    FSKernelCacheCoherencyTypeReadCache      writes bypass, reads cached
    FSKernelCacheCoherencyTypeWriteThrough   writes update cache and storage
    FSKernelCacheCoherencyTypeWriteBack      writes update cache, deferred

with `FSKernelCacheCoherencyAction` to push, invalidate, update or revoke when
the module needs the kernel to let go. That is a delegation protocol of the same
shape as SMB oplocks or NFSv4 delegations.

This is exactly what NFS could not offer and exactly why the attribute cache
experiment there failed: an NFS client invalidates its own cache after its own
writes and there is no way to tell it not to. A filesystem that knows it is
local and sole-writer, which this one is, can grant `WriteBack` and let the
kernel answer stats without an upcall reaching us at all. That is the mechanism
behind APFS's 0.70 us, and it is a caching property rather than a transport one.

> **Availability.** The caching APIs above are marked
> `FSKIT_API_AVAILABILITY_V3`, a later revision than the base FSKit protocols.
> They are present on macOS 27.0, where this was read. A module targeting the
> oldest FSKit does not get them, and without them FSKit is worth the small case
> rather than the large one.

### Rust can bind it directly, with no Swift

FSKit ships Objective-C headers in the Command Line Tools SDK, not only a Swift
module, so the bindings are ordinary `objc2` work. The Swift-to-Rust boundary
that would otherwise be the largest unknown in the port does not exist.

The protocol surface also maps closely onto what this crate already has:
`FSVolumeOperations` for lookup, create, remove and rename,
`FSVolumeReadWriteOperations`, `FSVolumeOpenCloseOperations`. And
`FSVolumeXattrOperations` means the AppleDouble sidecars stop existing rather
than being discarded.

### Packaging, which is already known to work on this machine

A module is an app extension, not a plain binary. The recipe, read from a
working one rather than from documentation:

* `<App>.app/Contents/Extensions/<Name>.appex`, with `CFBundlePackageType` of
  `XPC!`.
* `EXAppExtensionAttributes` in the appex `Info.plist`, carrying
  `EXExtensionPointIdentifier = com.apple.fskit.fsmodule` and an
  `FSPersonalities` dictionary whose `FSName` is the name `mount -t` uses.
* Registered through `pluginkit`, keyed by bundle identifier, and enabled by the
  user in System Settings. `pluginkit -m -p com.apple.fskit.fsmodule` lists
  installed modules with a leading `+` for enabled.
* `com.apple.developer.fskit.mount` is needed only to mount programmatically via
  `FSClient`; mounting through `mount -t` does not obviously require it.

This is not hypothetical here. `/Applications/FSKitExp.app` on this machine is
an Xcode-built FSKit module registered and enabled since 2026-07-25, so the
signing and enablement path is already retired.

### The open question is nix, not FSKit

`/Applications/FSKitExp.app` is a real directory, built by Xcode and installed
outside nix. On a machine whose configuration is rebuilt into fresh store paths,
the question is whether a nix-managed module survives:

1. Will LaunchServices register an app bundle under `/nix/store`, or a
   `/Applications` symlink pointing into it?
2. Is the enabled flag keyed on bundle identifier or on path? `pluginkit -m -vvv`
   shows a record carrying both a `Path` and a per-install `UUID`, so this is
   not answerable by inspection.

If the answer to the second is "path", then every `darwin-rebuild switch` that
changes the closure orphans the registration and the user re-enables their
filesystem by hand. Nobody uses a filesystem like that.

**The cheap probe needs no new bundle.** Move or reinstall the existing
`FSKitExp.app` to a different path, then run
`pluginkit -m -p com.apple.fskit.fsmodule` and see whether it still shows a
leading `+`. That answers the second question in about a minute and does not
require building or signing anything. Only if it survives is the first question
worth the longer experiment of installing from a store path and running a
`darwin-rebuild switch` across it.

There is a known-good fallback either way: build the module with Xcode and
install it to `/Applications` as a real bundle, outside nix, exactly as the
existing one is. Less pure, and demonstrably working today.

## Known limits

* A directory the revision contains cannot be removed or renamed away, so
  `rm -rf` of a tracked directory and `git clean -xfd` fail at the directory
  even after everything inside it has gone. Files and symlinks can be deleted;
  see the writable scratch layer section above.
* A deletion is scoped to the revision it was made against. Remounting the same
  scratch layer at a different revision brings every tracked name back, which
  is deliberate but will surprise anyone who deleted a file and then ran
  `jj new`.
* A held descriptor on a deleted tracked file still reads the revision's
  content, because the lower layer is still there behind the whiteout. That is
  what POSIX says an open unlinked file does, but over NFS it also means a
  client reusing a stale file handle reads a file it has already deleted.
* The FUSE transport is read-only. `jj fs mount --scratch` refuses it rather
  than silently downgrading, and its day-long attribute cache has to be shortened
  before anyone adds a write path there.
* Inode numbers are stable for the life of one mount, not across mounts. NFS file
  handles carry a per-startup generation number, so a restarted server answers a
  client's stale handle with `ESTALE` rather than with the wrong file.
* One transport process serves one revision. Serving several means several
  mounts.
* There are no hard links, no extended attributes and no device nodes, because a
  jj tree cannot express any of them.
* `df` against a FUSE mount reports a full filesystem with no free space, which
  is the only truthful answer for a tree that has no block accounting and cannot
  be written to. Over NFS, nfs3_server answers FSSTAT with a hardcoded tebibyte
  and does not consult us, so a writable NFS mount reports free space it does not
  necessarily have.
