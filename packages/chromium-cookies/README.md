# chromium-cookies

Extract and decrypt cookies from any macOS Chromium app (Chrome, Arc, Dia,
Brave, Edge, and Electron apps like Slack) so a local browser session can be
synced onto a remote ix VM.

Chromium apps share one SQLite `Cookies` schema but namespace the AES key per
app (a separate `<App> Safe Storage` Keychain entry each), so cookies cannot be
copied raw across apps or machines. This tool decrypts with the source app's key
so the result is replantable elsewhere.

```sh
chromium-cookies list                              # apps with a cookie store
chromium-cookies extract dia                       # decrypted, as JSON
chromium-cookies extract Slack --domain slack.com  # filter by host
chromium-cookies extract chrome --format netscape > cookies.txt
```

`--format netscape` writes a curl-importable `cookies.txt`, the shape a VM
wants. The first read of another app's secret pops the macOS Keychain dialog
once; choose "Always Allow" to silence it.

## Layers

| module     | job                                                             |
|------------|-----------------------------------------------------------------|
| `browser`  | find the `Cookies` DB (flat or `User Data/` layout), name the secret |
| `keychain` | read `<App> Safe Storage` via `/usr/bin/security`               |
| `crypto`   | PBKDF2 the key, AES-128-CBC decrypt the `v10`/`v11` blob         |
| `store`    | open the DB read-only + `immutable`, decrypt each row            |

## Scope

macOS only. Windows (DPAPI) and Linux (libsecret) use different key management;
add them as sibling `keychain`/`crypto` backends when needed.
