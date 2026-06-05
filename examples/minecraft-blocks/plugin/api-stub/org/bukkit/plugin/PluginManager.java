package org.bukkit.plugin;
import org.bukkit.event.Listener;
// Compile-only stub. The real interface is provided by the Paper server at runtime.
public interface PluginManager {
    void registerEvents(Listener listener, Plugin plugin);
}
