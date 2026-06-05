/**
  Buildable artifacts for the minecraft-blocks example.

  Curried on `{ ix, pkgs }` so each fleet node module can pull the binaries it
  needs from `pkgs` (available inside a NixOS module). Everything here builds
  offline in the Nix sandbox:

  - `plugin`: the Paper plugin jar, compiled with `javac` against a compile-only
    Bukkit API stub (see `plugin/api-stub/`) so the single event handler builds
    offline without Paper's churning transitive compile closure.
  - `loadFixtures`: the offline integration check. Loads the committed
    `fixtures.jsonl` records into a ClickHouse `local` table built from the
    shared schema, runs the bounding-box query, and asserts the expected count.
  - `mkQueryTool`: the `mc-blocks` ClickHouse query helper for the view node.
*/
{ ix, pkgs }:
let
  schema = import ./schema.nix { inherit (pkgs) lib; };
  inherit (pkgs) lib;

  # Compile-only Bukkit API stub. The plugin source is real Paper code, but
  # compiling it against the full Paper API jar drags in Paper's whole
  # transitive compile closure (adventure, Guava, examination, JetBrains
  # annotations), which churns across Paper builds and is brittle to pin. The
  # stub instead provides exactly the handful of symbols the single handler
  # touches, narrowed to the methods used. At runtime the server provides the
  # real classes (the stub is never shipped: the jar holds only the plugin's
  # own classes plus plugin.yml), so this is a `provided`-scope compile surface.
  # See plugin/api-stub/ for the stubbed types.
  pluginSrc = lib.fileset.toSource {
    root = ./plugin;
    fileset = lib.fileset.unions [
      ./plugin/src
      ./plugin/api-stub
    ];
  };

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
      }
      ''
        # Compile the API stub first, then the plugin against it. Only the
        # plugin's own classes and plugin.yml are packaged; the stub classes are
        # left out of the jar (the server provides the real ones at runtime).
        mkdir -p stub-classes classes
        javac --release ${jvmVersion} -d stub-classes \
          $(find "$src/api-stub" -name '*.java')
        javac --release ${jvmVersion} -cp stub-classes -d classes \
          $(find "$src/src/main/java" -name '*.java')
        cp -r "$src/src/main/resources/." classes/
        jar --create --file "$out" -C classes .
      '';

  # Offline integration check: committed fixtures -> ClickHouse local -> query.
  #
  # `clickhouse local` runs queries against on-disk MergeTree files with no
  # server and no network, so it exercises the real spatial schema (the Morton
  # ORDER BY from schema.nix, the signed-coordinate offset) inside the sandbox.
  # The fixtures (./fixtures.jsonl) place 32 records inside the bounding box and
  # 4 outside it, so the expected in-box count is a known constant.
  expectedInBox = 32;
  loadFixtures =
    pkgs.runCommand "minecraft-blocks-integration"
      {
        nativeBuildInputs = [ pkgs.clickhouse ];
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

        # The headline query: a 3D bounding box over the origin chunk column.
        # The Z-order ORDER BY means this scans contiguous granule ranges, not
        # the whole table. We assert the exact in-box count.
        in_box=$(run "
          SELECT count()
          FROM ${schema.fullTable}
          WHERE world = 'overworld'
            AND x >= 0 AND x < 16
            AND y >= 0 AND y < 16 + 64
            AND z >= 0 AND z < 16
        ")
        echo "rows in bounding box: $in_box (expected ${toString expectedInBox})"
        if [ "$in_box" != "${toString expectedInBox}" ]; then
          echo "FAIL: bounding-box count $in_box != ${toString expectedInBox}" >&2
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
        printf 'total=%s in_box=%s\n' "$total" "$in_box" > "$out/result"
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

        # Bounding-box query. Z-order ORDER BY lets ClickHouse prune granules
        # rather than scan the whole table.
        def "main box" [
          world: string
          x0: int y0: int z0: int
          x1: int y1: int z1: int
        ] {
          run $"SELECT count\(\) AS placements FROM ${schema.table} WHERE world = {world:String} AND x >= ($x0) AND x < ($x1) AND y >= ($y0) AND y < ($y1) AND z >= ($z0) AND z < ($z1)" $"--param_world=($world)"
        }

        # Per-chunk heatmap: 16x16 columns aggregated to chunk coordinates.
        def "main heatmap" [world: string --limit: int = 20] {
          run $"SELECT intDiv\(x, 16\) AS chunk_x, intDiv\(z, 16\) AS chunk_z, count\(\) AS placements FROM ${schema.table} WHERE world = {world:String} GROUP BY chunk_x, chunk_z ORDER BY placements DESC LIMIT ($limit)" $"--param_world=($world)"
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
