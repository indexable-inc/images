# Biff reading list

A Biff 2 Clojure web application on SQLite, running as a hardened non-root
systemd service in one private ix VM. Submit a title and a URL and the entry is
written to `/var/lib/biff-reading-list/reading-list.db`. The VM keeps ix's
rollback snapshot, so a bad deploy can be rolled back with the database intact.

## Run

```sh
# From this directory (examples/biff/reading-list in the index repo).
ix apply .#biff-reading-list
ix port-forward biff-reading-list 8080:8080
```

Get the repo with `git clone https://github.com/indexable-inc/index`.

Open `http://127.0.0.1:8080`.

## Shape

[`default.ix`](default.ix) is the whole example. It names the VM and sets
`services.biff-reading-list.enable = true`. The two pieces it reaches for live
outside this directory:

- [`modules/services/biff-reading-list`](../../../modules/services/biff-reading-list)
  is the service. It owns the `biff` system user, the systemd hardening, the
  claim on port 8080, the two deployment health checks, and the cookie secret
  generated into the state directory before first start. Set `port` or `host`
  on the module if 8080 is taken.
- [`packages/biff/reading-list`](../../../packages/biff/reading-list) is the
  application, resolved here as `pkgs.biff-reading-list` through the repo
  overlay. Its README covers the source layout and how to relock dependencies.

## Verify

`ix apply` reports the node healthy only once the unit is active and `/`
answers, so a successful apply already proves both. To look at the state
directory:

```sh
ix shell biff-reading-list -- ls -l /var/lib/biff-reading-list
```

`cookie-secret` is written 0400 on first start and reused across restarts, so
sessions survive a restart of the service.

The service authorizes every local write. Treat the VM as single-user and add
identity-aware rules before putting it anywhere shared.
