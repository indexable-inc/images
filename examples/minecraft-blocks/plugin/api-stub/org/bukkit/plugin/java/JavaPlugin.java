package org.bukkit.plugin.java;
import java.io.File;
import java.util.logging.Logger;
import org.bukkit.Server;
import org.bukkit.configuration.FileConfiguration;
import org.bukkit.plugin.Plugin;
// Compile-only stub of Paper's JavaPlugin base class, narrowed to the methods
// the example plugin overrides or calls. The real base class (and its full
// adventure/Guava transitive API) is provided by the server at runtime.
public abstract class JavaPlugin implements Plugin {
    public void onEnable() {}
    public void onDisable() {}
    public FileConfiguration getConfig() { throw new UnsupportedOperationException("stub"); }
    public File getDataFolder() { throw new UnsupportedOperationException("stub"); }
    public Server getServer() { throw new UnsupportedOperationException("stub"); }
    public Logger getLogger() { throw new UnsupportedOperationException("stub"); }
}
