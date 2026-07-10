pluginManagement {
    repositories {
        gradlePluginPortal()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositoriesMode = RepositoriesMode.FAIL_ON_PROJECT_REPOS
    repositories {
        // Every server is a standalone Gradle build (its Nix source root is its
        // own directory), so this repository-selection boilerplate is
        // intentionally identical across servers.
        // clone:ignore-start
        val ixMavenRepository = providers.gradleProperty("ix.mavenRepository")
        if (ixMavenRepository.isPresent) {
            maven {
                url = uri(ixMavenRepository.get())
                metadataSources {
                    gradleMetadata()
                    mavenPom()
                    artifact()
                }
            }
        } else {
            mavenCentral()
        }
        // clone:ignore-end
    }
}

rootProject.name = "minestom-hello"
