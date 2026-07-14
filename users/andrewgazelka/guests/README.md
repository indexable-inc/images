# Personal macOS guests

Data-only guest specs consumed by the general `macosGuests` module
(`modules/home/macos-guests.nix`, index `homeModules.macos-guests`), which
generates one idempotent `macos-guest-<name>` push command per guest.

## macos-primary — Beeper iMessage bridge (Linear ENG-7746)

vmkit guest at `~/.local/share/vmkit/guests/macos-primary`, auto-login user
`ix`, ssh `ix@192.168.64.6`. Declares:

- launchd agent `com.beeper.sh-imessage`: `bbctl run` for the `sh-imessage`
  bridge, KeepAlive, logging to `/tmp/imsg.log` on the guest.
- `~/.local/bin/bbctl`: the pinned bridge-manager CLI (`packages/bbctl`,
  v0.14.0 darwin arm64; hash in `packages/bbctl/pins.json`).

```sh
macos-guest-macos-primary          # idempotent apply
macos-guest-macos-primary status   # read-only drift report, exit 1 on drift
macos-guest-macos-primary ssh      # interactive guest shell
macos-guest-macos-primary ssh -- sw_vers  # run one guest command
```

### One-time bootstrap (stateful / GUI-gated, deliberately not applied)

General TCC mechanics: `modules/home/macos-guests/tcc-bootstrap.md`. The
bridge-specific steps, validated live 2026-07-14:

1. **`bbctl login`**: interactive bubbletea TUI; it wedges in headless PTYs,
   so run it in the guest's GUI Terminal.app. It writes the bridge
   registration under `~/Library/Application Support/bbctl/prod/sh-imessage/`
   (config.yaml + mautrix-imessage.db). That directory holds SECRETS and is
   the bridge's durable state: never commit it; include it in guest backups.
2. **Full Disk Access**: after the first apply, the agent crash-loops on
   `chat.db: operation not permitted` until bbctl auto-appears (toggled off)
   in System Settings > Privacy & Security > Full Disk Access; toggle it on.
3. **Contacts**: approve the prompt on first bridge start.
4. **Automation > Messages**: approve the prompt on the first outbound send.

### Verifying

`macos-guest-macos-primary` ends by printing the agent's launchd state
(`state = running`). On the guest, `/tmp/imsg.log` carries bridge output; an
outbound send logs a `step REMOTE, status SUCCESS` checkpoint, and the log
must stay free of `FTL` lines.
