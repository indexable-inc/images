pluginManagement {
    repositories {
        gradlePluginPortal()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositoriesMode = RepositoriesMode.FAIL_ON_PROJECT_REPOS
    repositories {
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
    }
}

rootProject.name = "minestom-spleef"
