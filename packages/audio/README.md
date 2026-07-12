# shared-audio: play together, send no audio

A LAN music session where peers never stream audio to each other. What
crosses the network is the *score* (a Loro CRDT: which instrument, its
controls, scheduled changes), the *instrument* (a content-hash-addressed
WASM module), and a *shared clock* (Ableton-Link-style offset estimation
over UDP pings). Every peer renders the same samples locally, bit-for-bit,
because every instrument is a pure function of the absolute shared frame.

CRDT = *what plays*. Clock = *when it plays*. Audio never leaves a machine.

## Crates (small, decoupled, trait-seamed)

| crate              | owns                                                            |
| ------------------ | --------------------------------------------------------------- |
| `audio-blob`       | content-hash-addressed module store (`BlobHash`, `BlobStore`)   |
| `audio-clock`      | shared timeline: NTP-style offset estimation, `SharedClock`, `MonotonicTime` seam |
| `audio-score`      | the Loro score: instrument ref, sparse controls, scheduled events |
| `audio-instrument` | wasmtime host for the sa ABI, determinism knobs, load-time validation |
| `audio-net`        | p2p node: TCP score gossip + blob transfer, UDP clock pings, leader election |
| `audio-engine`     | pure `Renderer` (bit-exact under any block split) + schedule-ahead `Player`, local `Volume` |
| `audio-app`        | the `shared-audio` binary: daemon, control CLI, macOS tray      |
| `audio-melody`     | example instrument in Rust; copy it to author your own          |
| `audio-e2e`        | two-node proof: publish -> converge -> bit-identical render     |

Local volume deliberately has no representation in the score (tests hold
that line): your volume keys never change what anyone else hears.

## Run it

```sh
nix run .#shared-audio -- daemon                 # first peer just plays
nix run .#shared-audio -- daemon --peer <host>:7648   # everyone else points at any peer
```

It sounds immediately: a fresh session seeds a built-in WAT instrument.
Peer discovery is static/injected on purpose; point peers at each other
(any one is enough, gossip meshes the rest via the score).

Then, from any machine in the session:

```sh
shared-audio status                    # peer id, shared frame, instrument hash
shared-audio volume up|down|mute       # local only, never shared
shared-audio set-control 0 440         # shared: everyone hears it
shared-audio publish my_instrument.wasm  # everyone switches, same shared frame
shared-audio tray                      # macOS menu-bar volume item
```

On Linux there is no menu bar; bind `shared-audio volume up|down` to media
keys in your compositor instead. Same daemon, same socket, same behavior.

## Run it as a service (launchd / systemd)

Use this repo's `homeModules.portable-services`, which renders one service
definition to both platforms:

```nix
{
  imports = [index.homeModules.portable-services];
  services.portable.shared-audio = {
    command = "${lib.getExe pkgs.shared-audio} daemon --peer studio.local:7648";
    environment.RUST_LOG = "info";
  };
}
```

State (score snapshot, module blobs, control socket) lives in
`~/.local/state/shared-audio/`; override the socket with
`$SHARED_AUDIO_SOCKET`.

## Drive it from the index kernel

The MCP kernel bundles a `sharedaudio` module speaking the same control
socket:

```python
import sharedaudio
sharedaudio.status()
sharedaudio.volume_down()
sharedaudio.publish("target/wasm32-unknown-unknown/release/audio_melody.wasm")
sharedaudio.schedule(at_frame=9_600_000, control=0, value=440.0)
```

## Author an instrument

Start from [`melody/`](melody/README.md). The contract is one rule:
*a pure function of the absolute shared frame*. The daemon validates every
published module against the sa ABI before it can reach any peer.

## Why it stays in sync

- **Determinism**: wasmtime with NaN canonicalization on and relaxed SIMD
  off; instruments get absolute frame numbers, so any block split renders
  identical bits (`audio-engine` and `audio-e2e` assert this with
  `f32::to_bits`).
- **Schedule ahead**: control changes land as events at future shared
  frames; every peer applies them at exactly that frame.
- **Clock**: the smallest peer id leads; followers estimate offset as the
  median of low-RTT ping samples and resync loudly if playback drifts.
