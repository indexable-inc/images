# Personal macOS guests

Data-only guest specs consumed by the general `macosGuests` module
(`modules/home/macos-guests.nix`, index `homeModules.macos-guests`), which
generates one idempotent `macos-guest-<name>` push command per guest.

## macos-primary — the agent's own iMessage identity (index#4360)

vmkit guest at `~/.local/share/vmkit/guests/macos-primary`, auto-login user
`ix`, ssh `ix@192.168.64.6`. Its Messages is signed into the agent's own
Apple ID, not the user's, so the agent can be added to a group chat and speak
as itself. Declares:

- launchd agent `dev.ix.agent-node`: a BEAM node the host kernel calls into
  over distributed Erlang, KeepAlive, logging to `/tmp/ix-agent-node.log`.
- `~/.local/bin/ix-agent-node`: the script that starts it.

```sh
macos-guest-macos-primary          # idempotent apply
macos-guest-macos-primary status   # read-only drift report, exit 1 on drift
macos-guest-macos-primary ssh      # interactive guest shell
macos-guest-macos-primary ssh -- sw_vers  # run one guest command
```

Sending, from a host cell:

```elixir
Imsg.send("+14155551212", "hi", mac: Mac.guest(:"ixagent@192.168.64.6"))
Imsg.send({:chat, guid}, "hi", mac: Mac.guest(:"ixagent@192.168.64.6"))
```

### Guest state this file cannot describe

Everything below lives only on the guest. A rebuilt guest needs it again.

- **The Apple ID sign-in**, through Messages' settings window. GUI-only, and
  two-factor sends a code to a trusted device.
- **`~/.erlang.cookie`**, the shared secret the host authenticates with. Not
  rendered from nix: a cookie in `/nix/store` is world-readable.
- **Homebrew's Elixir** at `/opt/homebrew/bin/elixir`, which the node script
  names. Nix cannot install on this guest: its installer cannot create
  `/etc/fstab` there even as root, so it cannot mount a store. Homebrew
  upgrades on its own schedule, so the runtime under the node can drift.
- **TCC grants** (`../../../modules/home/macos-guests/tcc-bootstrap.md`).
  Pre-seed them offline with `vmkit provision` against the stopped bundle
  rather than clicking through the guest GUI.

### TCC, the parts that cost a rebuild to learn

- **The grant follows the responsible process, not the tool.** Approving
  `/usr/bin/sqlite3` does nothing for a `sqlite3` run over ssh; approve
  `/usr/libexec/sshd-keygen-wrapper` and every ssh session inherits it.
- **The node's Apple Events grant names `erlexec`**, the launcher `elixir`
  execs, not `beam.smp`. Until it is granted, a send through the node hangs
  on an unanswered dialog on the guest screen rather than failing.
- **A grant for a non-Apple binary needs `--tcc-client-path`**: the
  requirement is computed from a copy staged on the host, but the row must
  name the path the guest runs.

### The guest is headless, so a sign-in needs a screen

`vmkit run-macos` opens no window. Screen Sharing supplies one, but the
service refuses to start until `screensharingd`, `ScreensharingAgent`,
`AppleVNCServer` and `ARDAgent` are all pre-approved for screen recording the
same offline way. With that done:

```sh
open "vnc://ix@192.168.64.6"   # add :<password> to skip the auth dialog
```

Auto-login lands on a locked screen unless
`/Library/Preferences/com.apple.loginwindow autoLoginUserScreenLocked` is
false; while it is locked, apps launch into a session behind the lock and
report no windows.

### Group chats reach the guest only after a message does

Adding the agent to a group delivers nothing to it. The thread appears on the
guest the first time someone sends a message to it, and `{:chat, guid}` can
address it only from then on. The guest does not use Messages in iCloud, so
there is no history to sync.
