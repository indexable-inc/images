# Multi-client file sharing

Standalone consumer example for sharing files across ix VMs over SMB with
correct POSIX locking semantics.

A `file-server` node exports `/var/lib/file-share` as the share named `share`.
Two client replicas (`client-0` and `client-1`) mount it at `/mnt/share` over
SMB 3.1.1. Linux's `cifs.ko` translates both `fcntl` byte-range locks and
`flock()` into native SMB byte-range locks, and `smbd` is configured to
mediate locks centrally, so locks coordinate across both clients.

## Run

```sh
# From the index repo root.
nix run .#multi-client-file-sharing-up
```

## Shape

- [`default.nix`](default.nix) defines the fleet: one server node and two
  client replicas with `dependsOn` so the server is up first.
- [`server.nix`](server.nix) configures Samba with the locking knobs
  (`strict locking`, `posix locking`, `kernel oplocks = no`,
  `strict sync = yes`) that keep two clients honest about each other's writes.
- [`client.nix`](client.nix) declares the CIFS mount with `vers=3.1.1` and
  leaves `nobrl` absent so byte-range locks stay enabled.

## Verify cross-client locking

Hold an exclusive `flock` on a file from one client:

```sh
ix shell client-0 -- flock -x /mnt/share/lockfile -c 'sleep 60'
```

From a second host shell, try to grab the same lock from the other client
non-blocking:

```sh
ix shell client-1 -- flock -nx /mnt/share/lockfile -c 'echo got-it'
```

The second invocation exits with status 1 until the first releases. Swap
`flock` for a `python -c 'import fcntl; fcntl.lockf(...)'` snippet to exercise
the same path via `fcntl` byte-range locks.

## Tradeoffs

- The share is **guest-writable** by default so the generated up wrapper works without secrets
  plumbing. Real deployments should drop `guest ok = yes` from
  [`server.nix`](server.nix), add a Samba user with `smbpasswd`, and pass
  `credentials=` to the CIFS mount through a systemd `LoadCredential` (the
  same shape [`python-daily-scraper`](../python-daily-scraper) uses for AWS
  keys).
- ix VMs share the host `linux-ix` kernel, so the SMB server has to be
  userspace `smbd` rather than in-kernel `ksmbd`. The client side still rides
  on `cifs.ko` from the host kernel.
- `strict sync = yes` plus `actimeo=1` trade some throughput for prompt
  cross-client visibility. Append-only or single-writer workloads can relax
  both.
