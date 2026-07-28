package dev.ix.minestom;

import net.minestom.server.MinecraftServer;
import net.minestom.server.coordinate.Pos;
import net.minestom.server.event.player.AsyncPlayerConfigurationEvent;
import net.minestom.server.instance.InstanceContainer;
import net.minestom.server.instance.block.Block;

public class Main {
    public static void main(String[] args) {
        MinecraftServer server = MinecraftServer.init();

        InstanceContainer instance = MinecraftServer.getInstanceManager().createInstanceContainer();
        // Generate every requested chunk as a layered flat world with grass on top at Y=39.
        instance.setGenerator(unit -> {
            var modifier = unit.modifier();
            modifier.fillHeight(0, 1, Block.BEDROCK);
            modifier.fillHeight(1, 36, Block.STONE);
            modifier.fillHeight(36, 39, Block.DIRT);
            modifier.fillHeight(39, 40, Block.GRASS_BLOCK);
        });

        MinecraftServer.getGlobalEventHandler().addListener(AsyncPlayerConfigurationEvent.class, event -> {
            // Put every joining player into that generated instance, slightly above the surface.
            event.setSpawningInstance(instance);
            event.getPlayer().setRespawnPoint(new Pos(0, 42, 0));
        });

        server.start("0.0.0.0", 25565);
    }
}
