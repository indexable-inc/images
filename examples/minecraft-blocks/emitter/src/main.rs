//! Emit Minecraft block-place domain facts as JSON Lines.
//!
//! One record per placement, one JSON object per line, written to stdout. The
//! object shape is byte-identical to what the Paper plugin writes for the same
//! placement, so this binary drives the log -> view pipeline end to end without
//! a running Minecraft server: pipe it into the Kafka topic, or load it into
//! ClickHouse directly for the integration check.
//!
//! Subcommands:
//!   fixtures   Emit a deterministic set of records. Some land inside a known
//!              bounding box, some outside it, so a downstream query can assert
//!              an exact count. This is what the example's integration check
//!              drives.

use std::fmt::Write as _;
use std::io::{self, Write};

/// A single block placement. The field order and JSON key names match the
/// `block_events` schema (`schema.nix`) and the Paper plugin's output exactly.
struct BlockEvent<'a> {
    world: &'a str,
    x: i32,
    y: i32,
    z: i32,
    block_type: &'a str,
    player_uuid: &'a str,
    player_name: &'a str,
    /// Milliseconds since the Unix epoch, UTC. ClickHouse `DateTime64(3)` reads
    /// this directly when the column is fed an integer millisecond value.
    timestamp_ms: i64,
}

impl BlockEvent<'_> {
    /// Serialize to one JSON Lines record (no trailing newline). The plugin
    /// produces the same bytes for the same placement; keep the two in lockstep.
    fn to_json_line(&self) -> String {
        let mut out = String::with_capacity(192);
        out.push('{');
        write!(out, "\"world\":{},", json_string(self.world)).unwrap();
        write!(out, "\"x\":{},", self.x).unwrap();
        write!(out, "\"y\":{},", self.y).unwrap();
        write!(out, "\"z\":{},", self.z).unwrap();
        write!(out, "\"block_type\":{},", json_string(self.block_type)).unwrap();
        write!(out, "\"player_uuid\":{},", json_string(self.player_uuid)).unwrap();
        write!(out, "\"player_name\":{},", json_string(self.player_name)).unwrap();
        write!(out, "\"timestamp\":{}", self.timestamp_ms).unwrap();
        out.push('}');
        out
    }
}

/// Minimal JSON string escaping: the only characters any field can carry that
/// need escaping are quote, backslash, and control characters. Block ids,
/// world names, and player names never contain raw control bytes, but escape
/// defensively so a crafted player name cannot break the line framing.
fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                write!(out, "\\u{:04x}", c as u32).unwrap();
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A handful of deterministic UUIDs and names so fixture output is stable
/// across runs (the integration check asserts exact counts).
const PLAYERS: [(&str, &str); 3] = [
    ("11111111-1111-4111-8111-111111111111", "Alice"),
    ("22222222-2222-4222-8222-222222222222", "Bob"),
    ("33333333-3333-4333-8333-333333333333", "Carol"),
];

/// Deterministic fixture set used by the integration check.
///
/// The check's bounding box is x,y,z in [0, 16) (one chunk column at the
/// origin). Exactly the records whose coordinates fall inside that box must be
/// returned by the bounding-box query. We place:
///   - a dense 4x2x4 = 32 record block fully inside the box, and
///   - a scattering of records well outside it (negative coords, far chunks),
/// so the expected in-box count is a known constant the check asserts against.
fn emit_fixtures(out: &mut impl Write) -> io::Result<()> {
    // A fixed base timestamp so output is byte-stable. 2026-01-01T00:00:00Z.
    let base_ms: i64 = 1_767_225_600_000;
    let mut seq: i64 = 0;
    let mut write_event = |out: &mut dyn Write,
                           world: &str,
                           x: i32,
                           y: i32,
                           z: i32,
                           block: &str,
                           player_idx: usize,
                           seq: &mut i64|
     -> io::Result<()> {
        let (uuid, name) = PLAYERS[player_idx % PLAYERS.len()];
        let ev = BlockEvent {
            world,
            x,
            y,
            z,
            block_type: block,
            player_uuid: uuid,
            player_name: name,
            timestamp_ms: base_ms + *seq * 1000,
        };
        *seq += 1;
        writeln!(out, "{}", ev.to_json_line())
    };

    // Inside the box: 4 (x) * 2 (y) * 4 (z) = 32 placements at the origin chunk.
    let blocks = ["minecraft:stone", "minecraft:dirt", "minecraft:oak_planks"];
    let mut inside = 0;
    for x in 0..4 {
        for y in 64..66 {
            for z in 0..4 {
                let block = blocks[(x + z) as usize % blocks.len()];
                let player_idx = (x + y + z) as usize;
                write_event(out, "overworld", x, y, z, block, player_idx, &mut seq)?;
                inside += 1;
            }
        }
    }
    debug_assert_eq!(inside, 32);

    // Outside the box: negative coordinates, a far chunk, and the nether. None
    // of these fall in x,y,z in [0, 16), so the bounding-box query must skip
    // them, and the Z-order ORDER BY lets ClickHouse prune their granules.
    write_event(out, "overworld", -100, 70, -100, "minecraft:cobblestone", 0, &mut seq)?;
    write_event(out, "overworld", 5000, 12, 5000, "minecraft:deepslate", 1, &mut seq)?;
    write_event(out, "overworld", 20, 65, 20, "minecraft:glass", 2, &mut seq)?;
    write_event(out, "nether", 0, 64, 0, "minecraft:netherrack", 0, &mut seq)?;

    Ok(())
}

fn main() -> io::Result<()> {
    let stdout = io::stdout();
    let mut out = io::BufWriter::new(stdout.lock());

    let mode = std::env::args().nth(1);
    match mode.as_deref() {
        Some("fixtures") => emit_fixtures(&mut out)?,
        Some(other) => {
            eprintln!("unknown subcommand: {other}");
            eprintln!("usage: block-events-emitter fixtures");
            std::process::exit(2);
        }
        None => {
            eprintln!("usage: block-events-emitter fixtures");
            std::process::exit(2);
        }
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_line_matches_schema_keys_in_order() {
        let ev = BlockEvent {
            world: "overworld",
            x: -1,
            y: 64,
            z: 2,
            block_type: "minecraft:stone",
            player_uuid: "11111111-1111-4111-8111-111111111111",
            player_name: "Alice",
            timestamp_ms: 1_767_225_600_000,
        };
        let line = ev.to_json_line();
        assert_eq!(
            line,
            "{\"world\":\"overworld\",\"x\":-1,\"y\":64,\"z\":2,\
             \"block_type\":\"minecraft:stone\",\
             \"player_uuid\":\"11111111-1111-4111-8111-111111111111\",\
             \"player_name\":\"Alice\",\"timestamp\":1767225600000}"
        );
    }

    #[test]
    fn player_name_with_quote_is_escaped() {
        let ev = BlockEvent {
            world: "w",
            x: 0,
            y: 0,
            z: 0,
            block_type: "minecraft:stone",
            player_uuid: "00000000-0000-4000-8000-000000000000",
            player_name: "a\"b",
            timestamp_ms: 0,
        };
        assert!(ev.to_json_line().contains("\"player_name\":\"a\\\"b\""));
    }

    #[test]
    fn fixtures_emit_thirty_six_records() {
        let mut buf = Vec::new();
        emit_fixtures(&mut buf).unwrap();
        let text = String::from_utf8(buf).unwrap();
        // 32 inside the box + 4 outside.
        assert_eq!(text.lines().count(), 36);
    }
}
