# iMessage bridge guest

Declarative half of the Beeper iMessage bridge (Linear ENG-7746) running in
the vmkit macOS guest: [`default.nix`](default.nix) renders the
`com.beeper.sh-imessage` launchd agent and installs `imessage-guest-apply`,
which pushes the pinned `bbctl` (`packages/bbctl`) and the plist to
the guest over ssh and (re)loads the agent. Interim consumer increment of
[index#2682](https://github.com/indexable-inc/index/issues/2682); once
`mkMacGuest` lands, the guest spec here migrates onto that seam.

## Usage

```nix
users.andrewgazelka.imessageGuest.enable = true;  # host/user default to ix@192.168.64.6
```

```sh
imessage-guest-apply
```

The apply is idempotent: rerun it after any bump of `packages/bbctl/pins.json`
(edit version/url, then `nix run .#update` re-pins the hash) or plist change.

## Manual bootstrap (once per guest)

Everything below is stateful or GUI-gated and deliberately not applied by nix.

1. **Guest**: vmkit macOS VM, signed into the Apple ID for iMessage, ssh
   reachable as `ix@192.168.64.6` with key auth.
2. **`bbctl login`**: interactive bubbletea TUI; it wedges in headless PTYs
   (nothing answers its `ESC[6n` cursor probes), so run it in the guest's GUI
   Terminal.app. This writes the bridge registration under
   `~/Library/Application Support/bbctl/prod/sh-imessage/` (config.yaml +
   mautrix-imessage.db). That directory holds SECRETS and is the bridge's
   durable state: never commit it, and include it in any guest backup.
3. **TCC grants**: GUI-only; macOS offers no supported non-MDM way to seed
   them (see index#2684 for the pre-seed machinery plan). Under launchd,
   `bbctl` is the TCC-responsible process, so grants made to Terminal.app do
   NOT carry over. After the first `imessage-guest-apply`:
   - **Full Disk Access**: the agent crash-loops on `chat.db: operation not
     permitted` until bbctl auto-appears (toggled off) in System Settings >
     Privacy & Security > Full Disk Access; toggle it on.
   - **Contacts**: approve the prompt on first bridge start.
   - **Automation > Messages**: approve the prompt on the first outbound send.

   All three persist across reboots and reapplies (the binary path is stable).

## Verifying

`imessage-guest-apply` ends by printing the agent's live pid. On the guest,
`/tmp/imsg.log` carries bridge output (block-buffered under launchd, so
silence is normal); an outbound send logs a `step REMOTE, status SUCCESS`
checkpoint.
