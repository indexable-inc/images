# Hermes Telegram Companion

The [Hermes operator VM](../hermes-agent/) tuned as a Telegram chat companion: same one-node, outbound-only shape, plus the long-poll Telegram platform and a chat-tuned persona. No inbound port, no webhook, no public hostname; the bot long-polls Telegram's API, so it works from any network the VM can dial out of.

## Shape

- [`ix.nix`](ix.nix) wraps the node as a one-node fleet.
- [`telegram.nix`](telegram.nix) layers two deltas on the shared [`../hermes-agent/hermes.nix`](../hermes-agent/hermes.nix) composition: `_module.args.hermes.telegram = true` and a chat-tuned `SOUL.md`.
- [`documents/SOUL.md`](documents/SOUL.md) is the persona: short messages, light markdown, conversation register. Everything else (model provider, memory, tools) is inherited from the hermes-agent preset and overridable through the same `_module.args.hermes` toggles.

## Create the bot (BotFather)

1. In Telegram, open a chat with [@BotFather](https://t.me/BotFather).
2. `/newbot`, pick a display name, then a username ending in `bot`. BotFather replies with the bot token (`123456789:ABC...`). That token is `TELEGRAM_BOT_TOKEN`.
3. Get your own numeric Telegram user ID: message [@userinfobot](https://t.me/userinfobot) (or any ID echo bot) and note the `id` it replies with.
4. Optional hygiene while you are there: `/setprivacy` -> Enable, so the bot only sees messages addressed to it in groups.

## Run

```sh
# From the index repo root.
nix run .#hermes-telegram-up
```

Then drop the secrets and restart the unit:

```sh
ix shell hermes -- sudo install -m0400 -o hermes -g hermes /dev/stdin /run/secrets/hermes.env <<'EOF'
OPENROUTER_API_KEY=sk-or-...
TELEGRAM_BOT_TOKEN=123456789:ABC...
TELEGRAM_ALLOWED_USERS=123456789
EOF
ix shell hermes -- sudo systemctl restart hermes-agent
```

`TELEGRAM_ALLOWED_USERS` is a comma-separated allowlist of numeric Telegram user IDs. It is the only authentication layer: anyone not on the list is ignored, and anyone on it talks to an agent with root in this VM. Keep it to IDs you control. To add a second user later, append the ID and restart the unit.

Now open your bot's chat in Telegram and say hello.

## Sample transcript

> **you:** hey, what machine are you on?
>
> **bot:** Hi! I'm on a NixOS VM called `hermes` — 1 vCPU bursting higher, systemd as PID 1, nushell for a shell. Root inside the guest, no access to the ix host.
>
> **you:** is anything eating cpu right now
>
> **bot:** Nothing dramatic. Top entry is `hermes-agent` itself at ~2%, everything else is idle. Load average 0.04.
>
> **you:** remind me to rotate the api key on friday morning
>
> **bot:** Done — cron job set for Friday 09:00 (VM time, UTC). I'll message you here when it fires.

## Notes

- The platform activates when `TELEGRAM_BOT_TOKEN` is present in the env file; the `telegram = true` toggle wires the env file through to the daemon. A VM booted without the token still comes up healthy and just stays off Telegram.
- Long-poll means outbound-only: nothing in this preset claims a port or needs `deployment.ipv4`.
- Everything in the [hermes-agent README](../hermes-agent/README.md) about secrets handling, provider swaps, and the trust model applies unchanged here.
