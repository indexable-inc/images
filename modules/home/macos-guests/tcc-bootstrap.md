# macOS guest TCC bootstrap (manual, once per guest)

Everything `macos-guest-<name>` pushes (launchd agents, binaries) is
declarative and idempotent. TCC — macOS privacy consent (Full Disk Access,
Contacts, Automation, ...) — is not: Apple offers no supported non-MDM way to
seed the grants, so a fresh guest needs a short GUI session. index#2684 tracks
pre-seeding the TCC db offline from the host; until that lands, follow this
page after the first apply.

## Rules that hold for any agent

- **Grants attach to the responsible process, not your shell.** Under
  launchd, the agent's executable is the TCC-responsible process, so grants
  made to Terminal.app while testing by hand do NOT carry over to the agent.
  Approve the prompts that fire while the agent runs under launchd.
- **A denied protected read often surfaces as a crash-loop, not a prompt.**
  Full Disk Access has no prompt: the first denied access (e.g. reading
  `chat.db`) fails with `operation not permitted`, the agent crash-loops
  under KeepAlive, and the binary auto-appears *toggled off* in
  System Settings > Privacy & Security > Full Disk Access. Toggle it on;
  the next respawn proceeds.
- **Per-service prompts fire on first use, not at load.** Contacts fires on
  the first address-book read, Automation > <target app> on the first Apple
  Event sent to that app. Keep the guest display open until each feature has
  fired once.
- **Interactive logins need the guest GUI.** TUI login flows (e.g. anything
  bubbletea-based) wedge in headless PTYs — nothing answers their `ESC[6n`
  cursor probes — so run one-time logins in the guest's GUI Terminal.app,
  never over plain ssh.
- **Grants persist** across reboots and reapplies as long as the binary's
  path stays stable. The apply command installs binaries at fixed paths
  (default `~/.local/bin/<name>`) for exactly this reason; content updates
  in place do not reset TCC.

## Checklist for a fresh guest

1. Run `macos-guest-<name>` from the host (pushes binaries + agents,
   bootstraps the gui launchd domain).
2. Open the guest display and log in as the auto-login user.
3. Perform each agent's one-time interactive login in GUI Terminal.app, if
   it has one. Login state usually lands under
   `~/Library/Application Support/<tool>/` — that is imperative guest state
   holding secrets; never copy it into Nix.
4. Trigger the agent's protected features once each; approve every prompt,
   and toggle on any binary that appeared disabled under Full Disk Access.
5. Back on the host, `macos-guest-<name> status` must report every resource
   in sync, and `launchctl print gui/<uid>/<label>` (over ssh) must show
   `state = running`.
