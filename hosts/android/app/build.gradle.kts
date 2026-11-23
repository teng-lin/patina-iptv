import groovy.json.JsonSlurper

plugins {
    id("com.github.jk1.dependency-license-report") version "3.1.5"
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// Relative to this Gradle root (hosts/android): the nsh checkout next to iptv-rs.
val nshJava: File = rootProject.file(providers.gradleProperty("nsh.java").get())
// Filled by tools/sync-host.sh from the NSH build of the Rust crate.
val nshOut = layout.buildDirectory.dir("nsh").get().asFile
val nshConfig = rootProject.file("../../crates/iptv-app/nsh.json")

// How `release` is signed: "distribution", "unsigned", "debug" or null (none
// chosen: its tasks fail rather than fall back to the debug key unasked).
val releaseSigning: String? = when {
    System.getenv("IPTV_UNSIGNED_RELEASE") == "1" -> "unsigned"
    System.getenv("KEYSTORE_PATH") != null -> "distribution"
    System.getenv("IPTV_DEBUG_SIGNED_RELEASE") == "1" -> "debug"
    else -> null
}

// Disposable validation can never replace the installed application, including
// when an operator accidentally points the test at a non-disposable device.
val validationAppId = System.getenv("IPTV_VALIDATION_APP_ID")
require(validationAppId == null || validationAppId == "com.hantekllc.iptvrs.validation") {
    "IPTV_VALIDATION_APP_ID must be com.hantekllc.iptvrs.validation"
}

android {
    namespace = "com.hantekllc.iptvrs.host"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.hantekllc.iptvrs"
        minSdk = 26
        targetSdk = 35
        versionCode = System.getenv("IPTV_ANDROID_VERSION_CODE")?.toInt()
            ?: System.getenv("GITHUB_RUN_NUMBER")?.toInt() ?: 1
        versionName = System.getenv("VERSION")
            ?: (JsonSlurper().parse(nshConfig) as Map<*, *>)["version"] as String
        ndk { abiFilters += listOf("x86_64", "arm64-v8a", "armeabi-v7a") }
    }

    signingConfigs {
        if (System.getenv("KEYSTORE_PATH") != null) {
            create("distribution") {
                storeFile = file(System.getenv("KEYSTORE_PATH"))
                storePassword = System.getenv("STORE_PASSWORD")
                keyAlias = System.getenv("KEY_ALIAS")
                keyPassword = System.getenv("KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        getByName("debug") {
            if (validationAppId != null) applicationIdSuffix = ".validation"
        }
        // What a viewer installs: not debuggable, so ART compiles the host's
        // and Media3's Java instead of interpreting it (a debuggable APK runs
        // it in the slow interpreter: seconds of a cold start on a TV box;
        // measured on a TV box). Signed with the distribution key
        // (KEYSTORE_PATH), or left unsigned for CI's separate signing job
        // (IPTV_UNSIGNED_RELEASE=1). The debug key only on request
        // (IPTV_DEBUG_SIGNED_RELEASE=1, a local install over a debug build);
        // with none of them a release task fails ([releaseSigning]).
        getByName("release") {
            isMinifyEnabled = false
            signingConfig = when (releaseSigning) {
                "distribution" -> signingConfigs.getByName("distribution")
                "debug" -> signingConfigs.getByName("debug")
                else -> null
            }
        }
        // `release` for measuring (tools/perf-android.sh): profileable from
        // the shell (src/profile/AndroidManifest.xml), so simpleperf works on
        // it, which a release must not be. Debug-signed, so it installs over
        // a debug build and keeps the app's data.
        create("profile") {
            initWith(getByName("release"))
            isDebuggable = false
            signingConfig = signingConfigs.getByName("debug")
            matchingFallbacks += listOf("release")
        }
    }

    sourceSets["main"].java.srcDirs("src/main/kotlin", "src/main/java", nshJava)
    sourceSets["main"].jniLibs.srcDirs(File(nshOut, "jniLibs"))
    sourceSets["main"].assets.srcDirs(File(nshOut, "assets"))

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }

    packaging { jniLibs { useLegacyPackaging = false } }
    lint {
        abortOnError = true
        checkReleaseBuilds = true
    }
}

val checkReleaseSigning by tasks.registering {
    val chosen = releaseSigning
    doLast {
        if (chosen == null) {
            throw GradleException(
                "No signing chosen for the release build. Set KEYSTORE_PATH (with " +
                    "STORE_PASSWORD, KEY_ALIAS, KEY_PASSWORD) to sign with the distribution " +
                    "key, IPTV_UNSIGNED_RELEASE=1 for an unsigned build, or " +
                    "IPTV_DEBUG_SIGNED_RELEASE=1 to sign it with the debug key for a " +
                    "local install.",
            )
        }
    }
}
// Every release build (assemble, bundle, install, lint) starts with preReleaseBuild.
tasks.matching { it.name == "preReleaseBuild" }.configureEach { dependsOn(checkReleaseSigning) }

dependencies {
    testImplementation("junit:junit:4.13.2")
    testImplementation("org.json:json:20250107")
    implementation("androidx.core:core:1.15.0")
    val media3 = "1.5.1"
    implementation("androidx.media3:media3-exoplayer:$media3")
    implementation("androidx.media3:media3-exoplayer-hls:$media3")
    implementation("androidx.media3:media3-exoplayer-dash:$media3")
    implementation("androidx.media3:media3-ui:$media3")
}

licenseReport {
    renderers = arrayOf(
        com.github.jk1.license.render.InventoryHtmlReportRenderer(),
        com.github.jk1.license.render.JsonReportRenderer(),
    )
    configurations = arrayOf("releaseRuntimeClasspath")
    outputDir = layout.buildDirectory.dir("reports/dependency-license").get().asFile.path
}
