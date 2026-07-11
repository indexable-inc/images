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
            maven("https://central.sonatype.com/repository/maven-snapshots") {
                mavenContent {
                    snapshotsOnly()
                }
            }
            mavenCentral()
        }
    }
}

rootProject.name = "minestom-examples"

include("servers:hello", "servers:spleef")
