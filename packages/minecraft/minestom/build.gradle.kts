subprojects {
    dependencyLocking {
        lockAllConfigurations()

        // Maven canonicalizes timestamped snapshots to master-SNAPSHOT, which
        // Gradle cannot lock. The version catalog and verification hashes pin it.
        ignoredDependencies.add("net.minestom:minestom")
    }
}
