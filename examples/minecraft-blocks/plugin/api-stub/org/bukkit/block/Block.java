package org.bukkit.block;
import org.bukkit.Material;
import org.bukkit.World;
// Compile-only stub. The real interface is provided by the Paper server at runtime.
public interface Block {
    World getWorld();
    int getX();
    int getY();
    int getZ();
    Material getType();
}
