// Maven Central answers GitHub's shared runners with HTTP 429. On GitHub Actions,
// ask Google's mirror of it first. gradle/verification-metadata.xml still checks
// every downloaded file, and Maven Central serves anything the mirror lacks.
pluginManagement {
    repositories {
        google()
        if (System.getenv("GITHUB_ACTIONS") == "true") {
            maven("https://maven-central.storage-download.googleapis.com/maven2/")
        }
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        if (System.getenv("GITHUB_ACTIONS") == "true") {
            maven("https://maven-central.storage-download.googleapis.com/maven2/")
        }
        mavenCentral()
    }
}

rootProject.name = "iptv-rs-android"
include(":app")
