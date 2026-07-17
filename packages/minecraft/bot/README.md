# mc-bot

A headless Minecraft client that joins a server (offline-mode login) and
records the session as a ReplayMod `.mcpr` replay.

```
mc-bot 127.0.0.1:25565 --protocol-version 776 --mc-version 26.2 \
    --record-seconds 10 --output session.mcpr
```

The Replay Mod records raw clientbound packets, and so does this bot: the
`.mcpr` file is a zip holding `recording.tmcpr` (per packet: a big-endian
u32 millisecond offset, a u32 length, then the raw uncompressed packet) and
`metaData.json` (protocol version, duration, server, generator). It is not
video and not world state — it is the client's view of the server, replayed.
Anything the server never sent (chunks out of range, entities never spawned)
simply does not exist in the replay, which is exactly what makes it the
right artifact for integration tests: open a failing run's replay in
ReplayMod and scrub through what the client actually saw.
tests/minestom-spleef-vm.nix records the spleef example server this way and
exports the replay from the VM.

The bot interprets as little as possible. It answers the packets that keep a
session alive — the configuration handshake (`SelectKnownPacks`,
`FinishConfiguration`), keep-alives, and pings — and records everything else
verbatim. Game content never gets parsed, so the recording is a faithful
byte-level trace no matter what the server does. Wire primitives (`VarInt`
framing, strings, address handling) come from mc-protocol
(packages/minecraft/protocol), the same single implementation
under mc-probe (Python) and mc-probe-kt (Kotlin); this crate adds the
compression layer, the login/configuration/play state machine, and the
`.mcpr` container.

Packet ids are per-state ordinals transcribed from the pinned Minestom's
packet registry (protocol 776 = Minecraft 26.2, in lockstep with
packages/minecraft/minestom/servers/spleef). A protocol bump moves them:
update `src/packets.rs` alongside the server pin, and pass the new
`--protocol-version`/`--mc-version` at the call sites.

Non-goals: online-mode encryption (the bot refuses servers that request it),
movement or gameplay (the bot stands still and watches), and replay
*reading* — ReplayMod and ReplayStudio own that side of the format.
