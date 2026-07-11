package dev.ix.minestom.spleef;

import net.minestom.server.MinecraftServer;
import net.minestom.server.event.player.AsyncPlayerConfigurationEvent;
import org.slf4j.Logger;
import org.slf4j.LoggerFactory;

/**
 * Spleef: players spawn on a floating disc of snow and dig it out from under
 * each other; falling through eliminates you, and the last player standing
 * wins. The server is one persistent {@link Lobby} that starts a fresh
 * {@link Game} — its own instance, event scope, and lifecycle — for every
 * round, so rounds can overlap and a finished arena is simply unregistered.
 */
public final class Main {
    private static final Logger LOGGER = LoggerFactory.getLogger(Main.class);

    public static void main(String[] args) {
        MinecraftServer server = MinecraftServer.init();

        Lobby lobby = new Lobby();
        // Every connection lands in the lobby; Game moves players out and back.
        MinecraftServer.getGlobalEventHandler().addListener(AsyncPlayerConfigurationEvent.class, event -> {
            event.setSpawningInstance(lobby.instance());
            event.getPlayer().setRespawnPoint(Lobby.SPAWN);
        });

        server.start("0.0.0.0", 25565);
        LOGGER.info("spleef server listening on :25565");
    }

    private Main() {}
}
