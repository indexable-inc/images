---
name: ssh-hosts
description: List the user's SSH host aliases (from ~/.ssh/config and its Includes) and recently used ssh targets. Use when connecting to a host, running a remote command over ssh, scp/rsync to a machine, or deciding which alias to use.
---

# SSH hosts

Run `ssh-hosts` to get the live list. It reads the machine fresh each time, so
the output is current rather than a snapshot baked into this skill:

```bash
ssh-hosts
```

It prints two sections:

- **SSH host aliases** parsed from `~/.ssh/config` and any `Include` files, with
  HostName / User / Port. Connect with the alias directly, e.g. `ssh hc1`.
- **Recent ssh commands** from shell history, newest first, showing what the
  user actually connects to.

Prefer an existing alias over a raw `user@host`: the alias already carries the
right HostName, User, and Port, so `ssh hc1` beats `ssh andrew@<ip> -p 9999`.
