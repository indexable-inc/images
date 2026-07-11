plugins {
    application
    java
}

group = "dev.ix"
version = "0.1.0"

base {
    archivesName = "minestom-hello"
}

dependencies {
    implementation(libs.minestom)
    implementation(libs.logback.classic)
}

java {
    toolchain {
        languageVersion = JavaLanguageVersion.of(25)
    }
}

application {
    mainClass = "dev.ix.minestom.Main"
}

tasks.withType<JavaCompile>().configureEach {
    options.release = 25
}

tasks.jar {
    duplicatesStrategy = DuplicatesStrategy.EXCLUDE
    manifest {
        attributes["Main-Class"] = application.mainClass.get()
    }
    from({
        configurations.runtimeClasspath.get().map { dependency ->
            if (dependency.isDirectory) dependency else zipTree(dependency)
        }
    })
}
