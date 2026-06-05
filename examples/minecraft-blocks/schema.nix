/**
  One source of truth for the block-place event.

  Every leg of the pipeline derives its field list from this file: the
  ClickHouse table DDL, the Kafka table-engine view, the topic name, and the
  documented record shape the Paper plugin and the Rust emitter both write.
  Editing a field here changes the table, the ingest view, and the assertions
  together, so the producer, the log, and the view can never drift apart.

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

  mortonFields = lib.sort (a: b: a.mortonAxis < b.mortonAxis) (lib.filter (f: f ? mortonAxis) fields);
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
  # so a 3D bounding box maps to a small set of contiguous granule ranges, then
  # falls back to time within a cell. `world` leads so each world is its own
  # contiguous run.
  createTableSql = ''
    CREATE TABLE IF NOT EXISTS ${database}.${table} (
      ${columnDefs}
    )
    ENGINE = MergeTree
    ORDER BY (world, ${mortonExpr}, timestamp)
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
    mortonExpr
    createDatabaseSql
    createTableSql
    kafkaColumnDefs
    ;

  # Column names in storage order. The loader's INSERT and the emitter's record
  # both key off this list, so a reordering or rename happens in one place.
  columnNames = map (f: f.name) fields;

  fullTable = "${database}.${table}";
}
