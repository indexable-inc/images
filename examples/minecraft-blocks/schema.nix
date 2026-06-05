/**
  One source of truth for the block-place event.

  The ClickHouse table DDL, the Kafka table-engine view, and the topic name are
  all generated from the field list below, so those three cannot drift. The
  Paper plugin writes the same record shape by hand (it is plain Java, not
  generated), so keep that one writer in lockstep with this file.

  A block placement is a domain fact. It carries a world, three signed block
  coordinates, the block type, who placed it, and when. It is not server
  telemetry, so it travels the log -> view path, never the OTel collector.
*/
{ lib }:
let
  # Minecraft coordinates are signed (negatives are legal), but ClickHouse
  # `mortonEncode` takes unsigned integers, so each axis is shifted into an
  # unsigned range by adding a fixed offset before encoding and subtracting it
  # after decoding.
  #
  # Bit budget: interleaving three axes into a single UInt64 curve value gives
  # each axis 21 bits (3 * 21 = 63 <= 64), so each shifted coordinate must fit
  # in [0, 2^21). The offset is 2^20, which centers a +/- 2^20 (about +/- 1.05
  # million) block window at the middle of the curve. That covers any normal
  # build area for the demo. A production deployment that needs the full +/- 30
  # million range partitions by region first (PARTITION BY the region/chunk),
  # then Morton-encodes within the bounded in-region offset, which is the same
  # idea applied per partition.
  #
  # This constant is load-bearing: the offset is declared once here and applied
  # identically by the table DDL, the loader query, and the round-trip check.
  coordOffset = 1048576; # 2^20

  # Columns in storage order. `mortonAxis` marks the three columns that are
  # interleaved into the Z-order curve; their order is the axis order passed to
  # mortonEncode (x, y, z), so range queries on any axis prune granules.
  fields = [
    {
      name = "world";
      chType = "LowCardinality(String)";
      doc = "World name, e.g. \"overworld\".";
    }
    {
      name = "x";
      chType = "Int32";
      mortonAxis = 0;
      doc = "Block X coordinate (signed).";
    }
    {
      name = "y";
      chType = "Int32";
      mortonAxis = 1;
      doc = "Block Y coordinate (signed).";
    }
    {
      name = "z";
      chType = "Int32";
      mortonAxis = 2;
      doc = "Block Z coordinate (signed).";
    }
    {
      name = "block_type";
      chType = "LowCardinality(String)";
      doc = "Namespaced block id, e.g. \"minecraft:stone\".";
    }
    {
      name = "player_uuid";
      chType = "UUID";
      doc = "Placing player's UUID.";
    }
    {
      name = "player_name";
      chType = "String";
      doc = "Placing player's name at placement time.";
    }
    {
      name = "timestamp";
      chType = "DateTime64(3, 'UTC')";
      doc = "Placement time, millisecond precision, UTC.";
    }
  ];

  database = "minecraft";
  table = "block_events";
  topic = "minecraft.block_events";

  # The example's canonical bounding box: a single half-open [lo, hi) interval
  # per axis (plus a world) that is the ONE definition of "the box". The fixture
  # generator places its dense in-box cluster from these same bounds, the
  # integration check derives its WHERE predicate from them, and the asserted
  # in-box count is whatever this box selects from the committed fixture. Keeping
  # the bounds here (read by both Nix and generate-fixtures.py from box.json)
  # means the three can never disagree: a fixture edit cannot quietly break the
  # in-box invariant by landing a row in a y-range one definition counts and
  # another does not.
  box = lib.importJSON ./box.json;

  # SQL predicate for `box`, built from the same field names. `axisPredicate`
  # emits a half-open `col >= lo AND col < hi` per axis so it matches the
  # generator's `lo <= v < hi` membership exactly. `mkBoxPredicate` takes the
  # world as a SQL fragment, so a query tool could pass a `{world:String}`
  # parameter while the integration check inlines the box's literal world via
  # `boxPredicate`.
  axisPredicate =
    axis:
    "${axis} >= ${toString (builtins.elemAt box.${axis} 0)} AND ${axis} < ${
      toString (builtins.elemAt box.${axis} 1)
    }";
  mkBoxPredicate =
    world:
    lib.concatStringsSep "\n  AND " (
      [ "world = ${world}" ]
      ++ map axisPredicate [
        "x"
        "y"
        "z"
      ]
    );
  boxPredicate = mkBoxPredicate "'${box.world}'";

  # Rows per granule. ClickHouse defaults to 8192, which would pack this demo's
  # few thousand rows into a single granule, leaving the skip indexes nothing to
  # prune. A small granule makes each granule's per-axis minmax box meaningful,
  # so a bounding-box query skips the granules that miss the box. A real table at
  # scale keeps the default; this is an example sized to show the mechanism.
  indexGranularity = 256;

  mortonFields = lib.sortOn (f: f.mortonAxis) (lib.filter (f: f ? mortonAxis) fields);
  axisCount = builtins.length mortonFields;

  # `mortonEncode((1,1,...), shifted_x, shifted_y, shifted_z)`. The mask tuple
  # (one `1` per axis) selects the equal-interleave Z-order curve, which is the
  # form that round-trips through `mortonDecode`. Each axis is cast to UInt32
  # after the offset shift so the encode sees the unsigned space.
  mortonMask = "(" + lib.concatStringsSep ", " (lib.genList (_: "1") axisCount) + ")";
  shiftedAxis = f: "toUInt32(${f.name} + ${toString coordOffset})";
  mortonExpr =
    "mortonEncode(${mortonMask}, " + lib.concatMapStringsSep ", " shiftedAxis mortonFields + ")";

  columnDefs = lib.concatMapStringsSep ",\n  " (f: "${f.name} ${f.chType}") fields;

  # minmax skip indexes on each coordinate axis. The Z-order ORDER BY keeps the
  # rows in each granule spatially compact, so each granule's per-axis [min, max]
  # is a tight box. ClickHouse cannot turn the bounding-box predicate into a key
  # range (mortonEncode is not monotonic per axis), but it CAN read these minmax
  # indexes and skip every granule whose box misses the query box. That is what
  # actually prunes a raw-coordinate range query; the ORDER BY alone only sorts
  # the data so the per-granule boxes stay tight. GRANULARITY 1 = one index entry
  # per granule, the finest skip resolution.
  skipIndexDefs = lib.concatMapStringsSep ",\n  " (
    f: "INDEX idx_${f.name} ${f.name} TYPE minmax GRANULARITY 1"
  ) mortonFields;

  # Ingest types for the Kafka engine table. ClickHouse recommends plain types
  # in a Kafka source table and letting the target table (and the implicit cast
  # in the materialized view's SELECT) apply storage encodings like
  # LowCardinality, so strip the LowCardinality wrapper for the queue.
  ingestType =
    chType:
    let
      m = builtins.match "LowCardinality\\((.*)\\)" chType;
    in
    if m == null then chType else builtins.head m;
  kafkaColumnDefs = lib.concatMapStringsSep ",\n  " (f: "${f.name} ${ingestType f.chType}") fields;

  # The view table. The sorting key linearizes (x, y, z) with the Z-order curve
  # so points close in space sort close on disk, keeping each granule spatially
  # compact; `world` leads so each world is its own contiguous run, and
  # `timestamp` last orders rows within a curve cell. The per-axis minmax skip
  # indexes then let a raw-coordinate bounding-box query skip granules whose box
  # misses the query box. The skip indexes do the pruning; the Z-order ordering
  # is what makes their per-granule boxes tight enough to prune well.
  #
  # ENGINE = ReplacingMergeTree makes replay idempotent. The ORDER BY tuple
  # (world, mortonEncode(x,y,z), timestamp) uniquely identifies a logical
  # placement: the same world cell cannot be placed twice at the same
  # millisecond, so the tuple is the natural key of the fact. ReplacingMergeTree
  # collapses rows that share the full sorting key down to one, and an exact
  # replay of a record is byte-identical (same coordinates, same player, same
  # timestamp), so it collapses to that one row. No version column is needed
  # because there is no "newer" copy to prefer; the copies are identical. This is
  # what turns an at-least-once transport plus this view into effectively-once:
  # the broker may re-deliver and the shipper may re-send the whole file on
  # restart, yet every duplicate folds back into the single canonical row.
  createTableSql = ''
    CREATE TABLE IF NOT EXISTS ${database}.${table} (
      ${columnDefs},
      ${skipIndexDefs}
    )
    ENGINE = ReplacingMergeTree
    ORDER BY (world, ${mortonExpr}, timestamp)
    SETTINGS index_granularity = ${toString indexGranularity}
  '';

  createDatabaseSql = "CREATE DATABASE IF NOT EXISTS ${database}";
in
{
  inherit
    fields
    mortonFields
    mortonMask
    database
    table
    topic
    coordOffset
    indexGranularity
    mortonExpr
    createDatabaseSql
    createTableSql
    kafkaColumnDefs
    box
    boxPredicate
    mkBoxPredicate
    ;

  # Column names in storage order. The loader's INSERT and the test fixture
  # both key off this list, so a reordering or rename happens in one place.
  columnNames = map (f: f.name) fields;

  fullTable = "${database}.${table}";
}
