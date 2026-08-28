import org.gradle.internal.os.OperatingSystem

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

/**
 * Version values come from gradle.properties, which is generated from the root
 * Cargo.toml. Nothing here may hard-code a version: ADR-008 makes Cargo.toml
 * the single source and `node tools/version.mjs check` fails the build on drift.
 */
val sonduitVersionName: String by project
val sonduitVersionCode: String by project
val sonduitMinSdk: String by project
val sonduitTargetSdk: String by project

/** The ABIs a release ships. Debug builds narrow this; see below. */
val releaseAbis = listOf("arm64-v8a", "armeabi-v7a", "x86_64")

android {
    namespace = "net.sonduit.app"
    compileSdk = sonduitTargetSdk.toInt()

    defaultConfig {
        applicationId = "net.sonduit.app"
        minSdk = sonduitMinSdk.toInt()
        targetSdk = sonduitTargetSdk.toInt()
        versionCode = sonduitVersionCode.toInt()
        versionName = sonduitVersionName

        ndk {
            abiFilters.addAll(releaseAbis)
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
        debug {
            // Building three ABIs for a debug run triples the Rust compile for
            // no benefit: a debug build is going onto one attached device.
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        compose = true
        // The settings screen shows the version, and ADR-008 forbids typing it
        // a second time. BuildConfig.VERSION_NAME carries the value that came
        // from gradle.properties, which came from the root Cargo.toml.
        buildConfig = true
    }

    sourceSets {
        getByName("main") {
            // The UniFFI generator writes Kotlin here, and cargo-ndk writes the
            // shared objects there. Both are build output and neither is
            // committed.
            kotlin.srcDir(layout.buildDirectory.dir("generated/uniffi"))
            jniLibs.srcDir(layout.buildDirectory.dir("generated/jniLibs"))
        }
    }

    packaging {
        // A stripped library is a third of the size and the symbols are not
        // useful without the matching Rust build anyway.
        jniLibs.keepDebugSymbols.add("**/libsonduit_ffi.so")
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2024.12.01")
    implementation(composeBom)

    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("androidx.lifecycle:lifecycle-service:2.8.7")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-graphics")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")

    // Reading the desktop's pairing QR code.
    //
    // CameraX rather than Camera2 directly: the preview and the analysis
    // stream are bound to a lifecycle and the rotation and format handling
    // that Camera2 leaves to the caller is exactly the part that is wrong on
    // one vendor's device and right on another's.
    val cameraX = "1.4.1"
    implementation("androidx.camera:camera-core:$cameraX")
    implementation("androidx.camera:camera-camera2:$cameraX")
    implementation("androidx.camera:camera-lifecycle:$cameraX")
    implementation("androidx.camera:camera-view:$cameraX")

    // ML Kit rather than ZXing, and the bundled model rather than the Play
    // services one. Pairing regularly happens over USB tethering or on a
    // network with no route to the internet, and the Play services variant
    // downloads its model on first use, so it would fail in exactly the
    // situation this feature exists for. The cost is about three megabytes.
    implementation("com.google.mlkit:barcode-scanning:17.3.0")

    // UniFFI's generated Kotlin depends on both of these by name.
    implementation("net.java.dev.jna:jna:5.15.0@aar")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")

    androidTestImplementation(composeBom)
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    testImplementation("junit:junit:4.13.2")
}

// ---------------------------------------------------------------------------
// Rust
// ---------------------------------------------------------------------------

val repoRoot: File = rootProject.projectDir.parentFile
val cargoBinary = if (OperatingSystem.current().isWindows) "cargo.exe" else "cargo"

/**
 * Cross-compiles sonduit-ffi for the ABIs this build needs.
 *
 * cargo-ndk rather than raw cargo: it sets the linker, sysroot and API level
 * for each ABI, which is exactly the part that is tedious to get right and
 * silently wrong when it is not.
 */
val cargoBuild by tasks.registering(Exec::class) {
    group = "rust"
    description = "Cross-compile the shared Rust for Android"

    val profile = if (project.hasProperty("rustRelease")) "release" else "debug"
    val abis = if (profile == "release") releaseAbis else listOf("arm64-v8a")
    val outputDir = layout.buildDirectory.dir("generated/jniLibs").get().asFile

    workingDir = repoRoot
    val arguments = mutableListOf("ndk")
    abis.forEach { abi ->
        arguments += "-t"
        arguments += abi
    }
    // The platform level is not optional. Without it cargo-ndk links against
    // the API 21 sysroot, where libaaudio.so does not exist yet, and the build
    // fails with an unfindable -laaudio.
    arguments += listOf("--platform", sonduitMinSdk)
    arguments += listOf("-o", outputDir.absolutePath, "build", "-p", "sonduit-ffi")
    if (profile == "release") {
        arguments += "--release"
    }

    commandLine(listOf(cargoBinary) + arguments)

    // Without this the task reruns on every build even when nothing changed,
    // and a Rust rebuild is the slowest thing in this project.
    inputs.dir(File(repoRoot, "crates"))
    inputs.file(File(repoRoot, "Cargo.toml"))
    outputs.dir(outputDir)
}

/**
 * Generates the Kotlin bindings from the built library.
 *
 * Reads the compiled `.so` rather than the source, because that is where the
 * UniFFI metadata ends up; running it against the source would need a second
 * parse that can disagree with what was actually compiled.
 */
val uniffiBindings by tasks.registering(Exec::class) {
    group = "rust"
    description = "Generate Kotlin bindings for sonduit-ffi"
    dependsOn(cargoBuild)

    val profile = if (project.hasProperty("rustRelease")) "release" else "debug"
    val library = File(repoRoot, "target/aarch64-linux-android/$profile/libsonduit_ffi.so")
    val outputDir = layout.buildDirectory.dir("generated/uniffi").get().asFile

    workingDir = repoRoot
    commandLine(
        cargoBinary,
        "run",
        "-p",
        "sonduit-ffi",
        "--bin",
        "uniffi-bindgen",
        "--",
        "generate",
        "--library",
        library.absolutePath,
        "--language",
        "kotlin",
        "--out-dir",
        outputDir.absolutePath,
    )

    inputs.file(library)
    outputs.dir(outputDir)
}

tasks.withType<org.jetbrains.kotlin.gradle.tasks.KotlinCompile>().configureEach {
    dependsOn(uniffiBindings)
}

tasks.named("preBuild") {
    dependsOn(uniffiBindings)
}
