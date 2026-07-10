# mc-protocol

Minecraft Java Edition wire-protocol primitives: VarInt encoding and packet
framing, length-prefixed strings, the Server List Ping (SLP) status
exchange, and legacy `§`/`&` format-code stripping for MOTD comparison.

The crate exists so the repo has exactly one implementation of the wire
format, shared by every language a health check or test harness is written
in:

- **Rust** — this crate (`mc_protocol::query`, `::ServerAddress`,
  `::strip_format_codes`, plus the `varint`/`wire` framing modules).
  mc-bot (packages/minecraft/bot), the headless replay-recording
  client, builds its packet layer on the same primitives.
- **Python** — `py/`, unibind-rendered bindings imported as `mc_protocol`;
  mc-probe (packages/minecraft/probe) is the primary consumer.
- **JVM** — `jvm/`, the same three-call surface rendered by unibind's jvm
  backend into C-ABI shims plus one generated Java class (`McProtocolJvm`)
  speaking the FFM API; mc-probe-kt (packages/minecraft/probe-kt)
  is the consumer.

The SLP exchange is deliberately small: handshake (next state = status) →
status request → status response carrying a JSON document → ping/pong round
for latency. `SlpStatus::from_status_json` is public so response parsing is
testable without a socket, and the loopback test in `src/slp.rs` exercises
the full exchange against an in-process server speaking the same framing
helpers.

Non-goal: SRV record lookup. In-repo health checks and tests address servers
by explicit `host:port`; a resolver indirection would only move failures
around. (mcstatus-based tooling used to do SRV lookups — if you need one,
resolve it before calling in.)
