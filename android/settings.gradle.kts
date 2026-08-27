// Repositories are declared here rather than per-project: Gradle 7 onward
// prefers it, and it keeps a stray module from silently resolving from
// somewhere else.
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "Sonduit"
include(":app")
