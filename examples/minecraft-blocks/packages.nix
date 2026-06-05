/**
  Buildable artifacts for the minecraft-blocks example.

  Curried on `{ ix, pkgs }` so each fleet node module can pull the binaries it
  needs from `pkgs` (available inside a NixOS module). Everything here builds
  offline in the Nix sandbox:

  - `plugin`: the Paper plugin jar, compiled with `javac` against the real Paper
    API jar (and the handful of transitive types `javac` needs in signatures),
    pinned by URL + SRI hash in `plugin/api-deps.json`. Compiling against the
    real API means class-vs-interface kinds and method descriptors are correct
    by construction, so the handler cannot hit a `NoSuchMethodError` or
    `IncompatibleClassChangeError` at runtime.
  - `loadFixtures`: the offline integration check. Loads the committed
    `fixtures.jsonl` records into a ClickHouse `local` table built from the
    shared schema, runs the bounding-box query, asserts the expected count, and
    proves the Z-order minmax skip indexes actually prune granules.
  - `mkQueryTool`: the `mc-blocks` ClickHouse query helper for the view node.
*/
{ ix, pkgs }:
let
  schema = import ./schema.nix { inherit (pkgs) lib; };
  inherit (pkgs) lib;

  pluginSrc = lib.fileset.toSource {
    root = ./plugin;
    fileset = ./plugin/src;
  };

  # The real Paper API and its compile-time transitive types, pinned by URL +
  # SRI hash in plugin/api-deps.json and fetched with the repo's shared
  # artifact intake. Compiling against the real API (not a hand-written stub)
  # makes class-vs-interface kinds and method descriptors correct by
  # construction: `Material` is the real enum, `NamespacedKey` the real final
  # class, `getConfig()` returns `org.bukkit.configuration.file.FileConfiguration`,
  # so the bytecode the handler emits matches what the server provides and there
  # is no `NoSuchMethodError`/`IncompatibleClassChangeError` at runtime.
  #
  # paper-api is pinned to the same build (26.1.2 build 64) the producer runs,
  # so the compile surface is exactly the runtime surface. The other entries are
  # the minimal set `javac` demands to resolve the inherited signatures of
  # `JavaPlugin`/`Listener` (adventure types on `Server`/`Audience`, JetBrains
  # nullability annotations, Guava `Multimap` on `Material`); they are
  # compile-only and are not shipped in the jar.
  apiClasspath = ix.artifacts.attachArtifactSources (lib.importJSON ./plugin/api-deps.json);
  apiJars = lib.mapAttrsToList (_: entry: entry.src) apiClasspath;

  # The repo's default JVM major (OpenJDK 25), which is what the Paper API and
  # the Minecraft server runtime target. Compiling against an older JDK fails on
  # the API jar's newer class-file version, so match it from the one source.
  jvmVersion = import ../../lib/languages/jvm-defaults.nix;
  pluginJdk = ix.languages.java.jdk pkgs {
    version = jvmVersion;
    distribution = "openjdk";
  };

  plugin =
    pkgs.runCommand "block-events-plugin.jar"
      {
        nativeBuildInputs = [ pluginJdk ];
        src = pluginSrc;
        classpath = lib.concatStringsSep ":" apiJars;
      }
      ''
        # Compile against the real API jars on the classpath. They are a
        # `provided`-scope surface: the server supplies these classes at
        # runtime, so only the plugin's own classes and plugin.yml are packaged.
        mkdir -p classes
        javac --release ${jvmVersion} -classpath "$classpath" -d classes \
          $(find "$src/src/main/java" -name '*.java')
        cp -r "$src/src/main/resources/." classes/
        jar --create --file "$out" -C classes .
      '';

  # Offline integration check: committed fixtures -> ClickHouse local -> query.
  #
  # `clickhouse local` runs queries against on-disk MergeTree files with no
  # server and no network, so it exercises the real spatial schema (the Morton
  # ORDER BY, the per-axis minmax skip indexes, the small granule, the
  # signed-coordinate offset, all from schema.nix) inside the sandbox.
  # `generate-fixtures.py` builds ./fixtures.jsonl with a dense in-box cluster
  # plus thousands of far-flung placements, so the in-box count is a known
  # constant AND the skip indexes have many distant granules to prune.
  expectedInBox = 512;

  # The headline bounding-box predicate over the origin chunk column. Declared
  # once so the asserted count and the EXPLAIN-pruning check run the identical
  # filter, and so the schema's source of truth is the only place coordinates live.
  boxPredicate = ''
    world = 'overworld'
      AND x >= 0 AND x < 16
      AND y >= 0 AND y < 16 + 64
      AND z >= 0 AND z < 16
  '';

  loadFixtures =
    pkgs.runCommand "minecraft-blocks-integration"
      {
        nativeBuildInputs = [
          pkgs.clickhouse
          pkgs.jq
        ];
      }
      ''
        export HOME="$TMPDIR"
        mkdir -p ch && cd ch

        cp ${./fixtures.jsonl} events.jsonl
        echo "loaded $(wc -l < events.jsonl) fixture records"

        run() {
          clickhouse local --path "$PWD/store" --multiquery "$1"
        }

        # Build the view exactly as the production table is built, from the one
        # schema source, then load the JSON Lines the plugin produces (mirrored
        # here by the committed fixture). JSONEachRow maps keys onto columns by name.
        run "${schema.createDatabaseSql}"
        run "${schema.createTableSql}"
        clickhouse local --path "$PWD/store" \
          --query "INSERT INTO ${schema.fullTable} FORMAT JSONEachRow" < events.jsonl

        total=$(run "SELECT count() FROM ${schema.fullTable}")
        echo "total rows: $total"

        # The headline query: a 3D bounding box over the origin chunk column. We
        # assert the exact in-box count.
        in_box=$(run "
          SELECT count()
          FROM ${schema.fullTable}
          WHERE ${boxPredicate}
        ")
        echo "rows in bounding box: $in_box (expected ${toString expectedInBox})"
        if [ "$in_box" != "${toString expectedInBox}" ]; then
          echo "FAIL: bounding-box count $in_box != ${toString expectedInBox}" >&2
          exit 1
        fi

        # Prove the skip indexes actually prune. EXPLAIN indexes=1 reports, per
        # index stage, how many granules survive. The PrimaryKey stage prunes
        # only by `world` (mortonEncode is not monotonic per axis, so a raw-axis
        # range cannot become a key range); the per-axis minmax skip indexes then
        # drop the granules whose [min, max] box misses the query box. The Z-order
        # ORDER BY is what keeps each granule's box tight enough to drop. Assert
        # that the best skip stage selects strictly fewer granules than the
        # PrimaryKey stage, i.e. the skip indexes genuinely cut work.
        explain=$(clickhouse local --path "$PWD/store" --query "
          EXPLAIN indexes = 1, json = 1
          SELECT count()
          FROM ${schema.fullTable}
          WHERE ${boxPredicate}
          FORMAT TSVRaw
        ")
        indexes=$(printf '%s' "$explain" | jq -c '[.. | objects | select(has("Indexes")) | .Indexes[]]')
        echo "index stages: $indexes"
        pk_granules=$(printf '%s' "$indexes" | jq '[.[] | select(.Type == "PrimaryKey") | ."Selected Granules"] | min')
        skip_granules=$(printf '%s' "$indexes" | jq '[.[] | select(.Type == "Skip") | ."Selected Granules"] | min')
        echo "granules after PrimaryKey: $pk_granules, after best skip index: $skip_granules"
        if [ -z "$skip_granules" ] || [ "$skip_granules" = "null" ]; then
          echo "FAIL: no skip index was consulted for the bounding-box query" >&2
          exit 1
        fi
        if [ "$skip_granules" -ge "$pk_granules" ]; then
          echo "FAIL: skip indexes did not prune ($skip_granules >= $pk_granules granules)" >&2
          exit 1
        fi

        # Prove the Morton round-trip: decoding the encoded curve value must
        # recover the original signed coordinates for a sampled row, including a
        # negative one. Uses the same mask form as the table's ORDER BY.
        roundtrip=$(run "
          WITH ${schema.mortonExpr} AS code
          SELECT
            toInt64(mortonDecode(${schema.mortonMask}, code).1) - ${toString schema.coordOffset} AS dx,
            toInt64(mortonDecode(${schema.mortonMask}, code).2) - ${toString schema.coordOffset} AS dy,
            toInt64(mortonDecode(${schema.mortonMask}, code).3) - ${toString schema.coordOffset} AS dz,
            (dx = x AND dy = y AND dz = z) AS ok
          FROM ${schema.fullTable}
          WHERE x = -100
          LIMIT 1
        ")
        echo "morton round-trip (dx dy dz ok): $roundtrip"
        case "$roundtrip" in
          *"	1") ;;
          *)
            echo "FAIL: morton decode did not recover signed coordinates: $roundtrip" >&2
            exit 1
            ;;
        esac

        mkdir -p "$out"
        cp events.jsonl "$out/"
        printf 'total=%s in_box=%s pk_granules=%s skip_granules=%s\n' \
          "$total" "$in_box" "$pk_granules" "$skip_granules" > "$out/result"
      '';

  # The ClickHouse query helper for the view node, mirroring ix-observe's shape.
  mkQueryTool =
    {
      host,
      port,
    }:
    ix.writeNushellApplication pkgs {
      name = "mc-blocks";
      runtimeInputs = [ pkgs.clickhouse ];
      meta.description = "Query the minecraft block_events spatial view in ClickHouse";
      text = ''
        let ch = [
          "client" "--host" "${host}" "--port" "${toString port}"
          "--database" "${schema.database}" "--format" "PrettyCompact"
        ]
        def run [sql: string, ...params: string] { ^clickhouse ...$ch ...$params --query $sql }

        def "main total" [] {
          run $"SELECT count() AS placements FROM ${schema.table}"
        }

        def "main top-players" [--limit: int = 10] {
          run $"SELECT player_name, count() AS placements FROM ${schema.table} GROUP BY player_name ORDER BY placements DESC LIMIT ($limit)"
        }

        # Bounding-box query. The per-axis minmax skip indexes (kept tight by the
        # Z-order ORDER BY) let ClickHouse skip granules rather than scan the
        # whole table.
        def "main box" [
          world: string
          x0: int y0: int z0: int
          x1: int y1: int z1: int
        ] {
          run $"SELECT count\(\) AS placements FROM ${schema.table} WHERE world = {world:String} AND x >= ($x0) AND x < ($x1) AND y >= ($y0) AND y < ($y1) AND z >= ($z0) AND z < ($z1)" $"--param_world=($world)"
        }

        # Per-chunk heatmap: 16x16 columns aggregated to chunk coordinates. A
        # Minecraft chunk is floor(coord / 16), so x = -1 is chunk -1, not 0;
        # intDiv truncates toward zero and would mis-bucket negative coordinates,
        # so divide as a float and floor.
        def "main heatmap" [world: string --limit: int = 20] {
          run $"SELECT toInt64\(floor\(x / 16\)\) AS chunk_x, toInt64\(floor\(z / 16\)\) AS chunk_z, count\(\) AS placements FROM ${schema.table} WHERE world = {world:String} GROUP BY chunk_x, chunk_z ORDER BY placements DESC LIMIT ($limit)" $"--param_world=($world)"
        }

        def "main sql" [...query: string] { run ($query | str join " ") }

        def main [] {
          print "subcommands: total, top-players, box, heatmap, sql"
        }
      '';
    };
in
{
  inherit
    schema
    plugin
    loadFixtures
    mkQueryTool
    expectedInBox
    ;
}
