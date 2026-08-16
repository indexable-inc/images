---
name: kernel-upstream
description: "Read upstream Linux from the CLI: search lore.kernel.org, fetch a patch series by message-id with b4, and find which released tag first carried a fix. Use when a task names a CVE, a kernel mitigation, a patch series, a lore thread, or asks whether the fleet's kernel already carries a fix."
---

## Reading upstream Linux without a browser

Three different questions, three different tools. Picking the wrong one is how
this turns into browsing.

| Question | Tool |
| --- | --- |
| Was this posted or discussed? | lore search over HTTPS |
| Give me the patches | `b4` by message-id |
| Which released kernel has it? | a local clone, `git tag --contains` |

lore can never answer the third one. A series on the list may be unmerged,
merged to a maintainer tree, or in mainline, and the mail thread looks the same
in all three cases.

### Search lore, and send a User-Agent

lore.kernel.org answers curl's default User-Agent with **HTTP 403**. Set one, or
every query returns an error page that an entry count reads as zero results.

```sh
curl -sS -A "ix-agent/1.0 (kernel patch lookup; andrew@ix.dev)" \
  "https://lore.kernel.org/all/?q=dfn%3Akernel%2Flivepatch%2Fcore.c&x=A&o=-1" \
  -o /tmp/lore.atom -w 'http=%{http_code}\n'
```

`x=A` gives Atom, `x=m` gives a gzipped mbox, `o=-1` sorts newest first. Check
the status code, not just the entry count.

The `q=` value is public-inbox search syntax, which is the part worth knowing:

- `dfn:path/to/file.c` — patches touching that file. The precise one.
- `dfs:"some code"` — text inside a diff hunk.
- `s:subject`, `f:author`, `nq:"free text"`
- `AND`, `OR`, `NOT`, and `d:20260201..` for a date floor.

### Fetch a series with b4

`b4` is on PATH. Give it a message-id, no leading `<`:

```sh
mkdir -p /tmp/b4-out   # b4 will NOT create this, see below
b4 -n mbox -o /tmp/b4-out 20250226184540.2250357-1-derkling@google.com
b4 am  <msgid>         # ready-to-apply patches, newest revision of the series
b4 shazam <msgid>      # apply straight into the current git tree
```

`-n` skips the lookup cache. `b4 mbox -o <dir>` fetches the thread and then dies
with a `FileNotFoundError` traceback if the directory does not exist, after the
network work and after printing "N messages in the thread", so the run looks
like it worked until you check for the file (ENG-12940). `mkdir -p` first.

`lei`, public-inbox's own query client, does not work on this Mac: it needs
`AF_UNIX` `SOCK_SEQPACKET`, which macOS lacks, and fails with `socket: Protocol
not supported` before it reaches the network. Do not reach for it on darwin.

### Which release contains a fix

This is the question that decides whether a fleet kernel bump is worth anything,
and it needs a clone rather than mail:

```sh
git clone --filter=blob:none --bare https://github.com/torvalds/linux.git /tmp/linux.git
git -C /tmp/linux.git log --oneline --all --grep='<term>' | head
git -C /tmp/linux.git tag --contains <sha> | sort -V | head -3
```

`--filter=blob:none --bare` still costs 2.0 GB and several minutes, so start it in
the background and do something else. Worked example, the Safe-RET interrupt
injection fix on 2026-08-06:

```
$ git -C /tmp/linux.git log --oneline -1 7e7f81cf6f5c
7e7f81cf6f5c x86/bugs: Make Safe-RET robust against interrupt injection
$ git -C /tmp/linux.git tag --contains 7e7f81cf6f5c
$
```

**Empty output is the answer, not a failed command:** merged to mainline, in no
released tag yet. That one line is what said a 7.1.4 to 7.1.6 bump would have
bought nothing. Check the stable branch and the queue separately before
concluding a point release will carry it, since neither follows from mainline:

```sh
git -C /tmp/linux.git remote add stable https://git.kernel.org/pub/scm/linux/kernel/git/stable/linux.git
git -C /tmp/linux.git log --oneline stable/linux-7.1.y --grep='<subject>'
git clone --depth 1 https://git.kernel.org/pub/scm/linux/kernel/git/stable/stable-queue.git /tmp/stable-queue
```

No `Cc: stable` on the commit and nothing in `queue-<version>` means no backport
is coming automatically.

Then compare against what the fleet actually runs, which is the nixpkgs pin and
not the newest tag:

```sh
nix eval --raw "github:NixOS/nixpkgs/<rev>#linuxPackages_latest.kernel.version"
```

Compute hosts take `pkgs.linuxPackages_latest` and storage hosts take
`pkgs.linuxPackages` (`nix/modules/roles/ovh-{compute,stor}.nix` in ix), so a fix
can be present on one role and absent on the other. The two roles are not even
the same vendor: compute is AMD Zen 5, hil-stor-2 is an Intel Xeon.

### A mitigation is only live if the kernel selected it

Before pricing a fix, read whether the host runs the thing being fixed. Most
speculative-execution fixes are `ALTERNATIVE`-gated on a CPU feature, so they
patch in on some parts and are absent on others:

```sh
grep -H . /sys/devices/system/cpu/vulnerabilities/*
```

Safe-RET is the worked example again: the fix above only applies where
`spec_rstack_overflow` reads `Mitigation: Safe RET`. Every ix compute host reads
`Mitigation: Reduced Speculation` instead, which took one command and settled a
question a version bump could not.

### The guest kernel is a different kernel

`views/linux` and `packages/linux-ix` in ix build the kernel that boots ix VMs,
pinned well behind the host at a `-rc` of its own. A host CPU-mitigation
question is answered by the nixpkgs pin; only a guest-visible bug is answered by
the view. Do not bump one believing you fixed the other.
