package dev.ix.example.blockevents;

import java.io.IOException;
import java.io.UncheckedIOException;
import java.io.Writer;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import org.bukkit.World;
import org.bukkit.block.Block;
import org.bukkit.entity.Player;
import org.bukkit.event.EventHandler;
import org.bukkit.event.Listener;
import org.bukkit.event.block.BlockPlaceEvent;
import org.bukkit.plugin.java.JavaPlugin;

/**
 * Emits one block-place domain fact per placement as a JSON Lines record.
 *
 * <p>A block placement is a domain fact, not server telemetry, so it goes to
 * the durable log, never the OTel collector. This plugin appends each record to
 * a file; the Kafka transport (see the example module) tails that file and
 * produces it to the {@code minecraft.block_events} topic, which materializes
 * into the ClickHouse spatial view.
 *
 * <p>The record shape matches the {@code block_events} schema. That field list
 * lives once in {@code schema.nix} and generates the ClickHouse table, the Kafka
 * ingest view, and the topic; this writer is plain Java and is not generated, so
 * keep it in lockstep with that file.
 */
public final class BlockEventsPlugin extends JavaPlugin implements Listener {

    private Writer log;

    @Override
    public void onEnable() {
        // Path is supplied by the deployment so the Kafka producer and the
        // plugin agree on where records land. Falls back to the plugin data
        // folder when unset, which keeps a hand-run server working.
        String configured = getConfig().getString("logPath");
        Path logPath =
                configured != null
                        ? Path.of(configured)
                        : getDataFolder().toPath().resolve("block-events.jsonl");
        try {
            Files.createDirectories(logPath.getParent());
            this.log =
                    Files.newBufferedWriter(
                            logPath,
                            StandardCharsets.UTF_8,
                            StandardOpenOption.CREATE,
                            StandardOpenOption.APPEND);
        } catch (IOException e) {
            throw new UncheckedIOException("cannot open block-event log " + logPath, e);
        }
        getServer().getPluginManager().registerEvents(this, this);
        getLogger().info("block-events: logging placements to " + logPath);
    }

    @Override
    public void onDisable() {
        if (log != null) {
            try {
                log.close();
            } catch (IOException ignored) {
                // Shutting down; nothing useful to do with a close failure.
            }
        }
    }

    @EventHandler
    public void onBlockPlace(BlockPlaceEvent event) {
        Block block = event.getBlockPlaced();
        Player player = event.getPlayer();
        String record =
                "{"
                        + "\"world\":" + jsonString(dimensionName(block.getWorld())) + ","
                        + "\"x\":" + block.getX() + ","
                        + "\"y\":" + block.getY() + ","
                        + "\"z\":" + block.getZ() + ","
                        + "\"block_type\":" + jsonString(blockType(block)) + ","
                        + "\"player_uuid\":" + jsonString(player.getUniqueId().toString()) + ","
                        + "\"player_name\":" + jsonString(player.getName()) + ","
                        + "\"timestamp\":" + System.currentTimeMillis()
                        + "}";
        try {
            log.write(record);
            log.write('\n');
            log.flush();
        } catch (IOException e) {
            getLogger().warning("block-events: failed to write record: " + e.getMessage());
        }
    }

    /** Namespaced block id, e.g. {@code minecraft:stone}. */
    private static String blockType(Block block) {
        return block.getType().getKey().toString();
    }

    /**
     * Stable dimension name for the schema, derived from the world's environment
     * rather than its folder name.
     *
     * <p>The query schema keys on stable dimension names ("overworld", "nether",
     * "the_end"), but a Bukkit {@code World#getName()} is the on-disk folder name
     * and varies with {@code level-name} (this deployment uses "blocks"). Reading
     * {@link World#getEnvironment()} instead makes a real placement match the
     * schema regardless of how the server folders are named. A {@code CUSTOM}
     * world has no canonical dimension name, so it falls back to its world name.
     */
    private static String dimensionName(World world) {
        return switch (world.getEnvironment()) {
            case NORMAL -> "overworld";
            case NETHER -> "nether";
            case THE_END -> "the_end";
            default -> world.getName();
        };
    }

    /** Minimal JSON string escaping for the record fields. */
    private static String jsonString(String s) {
        StringBuilder out = new StringBuilder(s.length() + 2);
        out.append('"');
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '"' -> out.append("\\\"");
                case '\\' -> out.append("\\\\");
                case '\n' -> out.append("\\n");
                case '\r' -> out.append("\\r");
                case '\t' -> out.append("\\t");
                default -> {
                    if (c < 0x20) {
                        out.append(String.format("\\u%04x", (int) c));
                    } else {
                        out.append(c);
                    }
                }
            }
        }
        out.append('"');
        return out.toString();
    }
}
