#!/usr/bin/env python3
"""Generate the committed block-placement fixture for the integration check.

The fixture mirrors exactly what the Paper plugin writes: one JSON Lines record
per placement, same field order and types (see schema.nix and the plugin). It is
committed as ``fixtures.jsonl`` so the whole log -> view path is testable offline
with no server. Regenerate and commit when the layout below changes::

    ./generate-fixtures.py > fixtures.jsonl

The layout is built to demonstrate the Z-order minmax skip indexes, not just to
hold a row count:

- A dense cluster of in-box placements inside the origin chunk column, filling
  the box's x/z footprint at the two lowest y layers. With the Morton ORDER BY
  these land together in a few granules, so the bounding-box query reads only
  those granules.
- Thousands of placements scattered far across the overworld, deliberately far
  from the box, so most granules carry per-axis ranges that miss the box and are
  skipped. This is what makes ``read_rows`` far smaller than a full scan.
- A handful of nether placements and one negative-coordinate placement, so the
  per-dimension name path and the signed-coordinate Morton round-trip are both
  exercised.

The bounding box is NOT defined here. It lives once in ``box.json`` next to
``schema.nix``; this generator, the Nix schema, and the integration-check
predicate all read those same bounds, so the in-box region cannot drift between
the data and the query. The in-box count is a deterministic constant the check
asserts exactly; it is whatever the shared box selects from the rows below.
"""

import json
import pathlib
import sys

# Three stable players, matching the prose in the README. Index 0..2.
PLAYERS = [
    ("11111111-1111-4111-8111-111111111111", "Alice"),
    ("22222222-2222-4222-8222-222222222222", "Bob"),
    ("33333333-3333-4333-8333-333333333333", "Carol"),
]

# Block palette for variety; choice is irrelevant to the spatial query.
BLOCKS = [
    "minecraft:stone",
    "minecraft:dirt",
    "minecraft:oak_planks",
    "minecraft:cobblestone",
    "minecraft:glass",
    "minecraft:bricks",
]

# Placement clock: a fixed epoch (2026-01-01T00:00:00Z) plus a per-row second so
# timestamps are stable and ordered.
BASE_MS = 1767225600000

# The ONE bounding box, read from box.json next to schema.nix. The Nix schema
# and the integration-check predicate read the same file, so "the box" has a
# single definition that the data and the query share. Axes are half-open
# [lo, hi) intervals; membership is `lo <= v < hi`, matching the SQL the schema
# derives (`v >= lo AND v < hi`).
BOX = json.loads((pathlib.Path(__file__).parent / "box.json").read_text())


def record(seq: int, world: str, x: int, y: int, z: int) -> dict:
    player_uuid, player_name = PLAYERS[seq % len(PLAYERS)]
    return {
        "world": world,
        "x": x,
        "y": y,
        "z": z,
        "block_type": BLOCKS[seq % len(BLOCKS)],
        "player_uuid": player_uuid,
        "player_name": player_name,
        "timestamp": BASE_MS + seq,
    }


def main() -> None:
    rows = []
    seq = 0

    world = BOX["world"]
    (x0, x1), (y0, _y1), (z0, z1) = BOX["x"], BOX["y"], BOX["z"]

    # 1) Dense in-box cluster: the full x/z footprint of the box at its two
    # lowest y layers (y0 and y0+1), so every placement is inside BOX by
    # construction. The two-layer height keeps the cluster a flat slab the Morton
    # order packs into a few granules. Count = width(x) * width(z) * 2.
    for y in (y0, y0 + 1):
        for x in range(x0, x1):
            for z in range(z0, z1):
                rows.append(record(seq, world, x, y, z))
                seq += 1

    # 2) Far-flung overworld placements on a coarse grid well outside the box, so
    # their granules' per-axis ranges miss the box. 49 * 49 = 2401 rows minus the
    # in-box column the loop skips.
    far = list(range(-4800, 4801, 200))  # excludes the in-box column
    for x in far:
        for z in far:
            if x0 <= x < x1 and z0 <= z < z1:
                continue  # never collide with the in-box column
            rows.append(record(seq, world, x, 72, z))
            seq += 1

    # 3) Nether placements: a small cluster, to exercise the dimension-name path.
    for x in range(8):
        for z in range(8):
            rows.append(record(seq, "nether", x, 32, z))
            seq += 1

    # 4) One deep-negative-coordinate placement for the signed Morton round-trip.
    rows.append(record(seq, "overworld", -100, 70, -100))
    seq += 1

    in_box = sum(
        1
        for r in rows
        if r["world"] == BOX["world"]
        and BOX["x"][0] <= r["x"] < BOX["x"][1]
        and BOX["y"][0] <= r["y"] < BOX["y"][1]
        and BOX["z"][0] <= r["z"] < BOX["z"][1]
    )
    # Stamp the invariant the check asserts so a layout change is caught here too.
    print(
        f"# in_box={in_box} total={len(rows)}",
        file=sys.stderr,
    )

    out = sys.stdout
    for r in rows:
        out.write(json.dumps(r, separators=(",", ":")))
        out.write("\n")


if __name__ == "__main__":
    main()
