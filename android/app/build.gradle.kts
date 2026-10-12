import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

// The native side (crates/android) is cross-compiled with the NDK and cargo-ndk, neither of
// which a plain CI runner has. CI builds the APK with `-Psh705x.rust=false`, so the Kotlin
// and the resources are still compiled, and the Rust is checked by the workspace's own job.
// Locally it stays on; off, the tasks below are never configured, so a missing NDK cannot
// fail configuration either.
val buildRust = (findProperty("sh705x.rust") as String?)?.toBoolean() ?: true

android {
    namespace = "com.varuna.sh705x"
    compileSdk = 37
    if (buildRust) ndkVersion = "29.0.14206865"

    defaultConfig {
        applicationId = "com.varuna.sh705x"
        minSdk = 26
        targetSdk = 36
        versionCode = 5
        versionName = "0.3.2"
        androidResources.localeFilters += setOf("en", "es")
    }

    splits {
        abi {
            isEnable = true
            reset()
            include("arm64-v8a", "x86_64")
            isUniversalApk = false
        }
    }

    // Per-app language needs every language in the installed APK.
    bundle {
        language {
            @Suppress("UnstableApiUsage")
            enableSplit = false
        }
    }

    lint {
        disable += "OldTargetApi"
        abortOnError = false
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    sourceSets {
        getByName("main") {
            jniLibs.directories.add("src/main/jniLibs")
        }
    }

    buildFeatures {
        compose = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_21
        targetCompatibility = JavaVersion.VERSION_21
    }
}

kotlin {
    jvmToolchain {
        languageVersion.set(JavaLanguageVersion.of(21))
        vendor.set(JvmVendorSpec.AMAZON)
    }
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_21)
    }
}

dependencies {
    // Frame decoding is plain Kotlin, so it is tested on the host.
    testImplementation(libs.junit)

    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.graphics)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.compose.material3)
}

// --- The Rust side: crates/android, built for each ABI the APK splits into ---

val targets = listOf("arm64-v8a", "x86_64")

val isWindows = System.getProperty("os.name").lowercase().contains("windows")
val cargoName = if (isWindows) "cargo.exe" else "cargo"
val cargoHome = System.getenv("CARGO_HOME") ?: (System.getProperty("user.home") + "/.cargo")
val cargoPath = file("$cargoHome/bin/$cargoName")
val cargo: String = if (cargoPath.exists()) cargoPath.absolutePath else cargoName
val jniLibs = file("src/main/jniLibs")

if (buildRust) {
    targets.forEach { target ->
        tasks.register<Exec>("buildRust_$target") {
            group = "rust"
            description = "Builds crates/android for $target with cargo-ndk"
            // The Cargo workspace is the repository's root.
            workingDir = file("../..")
            val components = project.extensions
                .getByType<com.android.build.api.variant.ApplicationAndroidComponentsExtension>()
            environment("ANDROID_NDK_HOME", components.sdkComponents.ndkDirectory.get().asFile.absolutePath)
            environment("ANDROID_HOME", components.sdkComponents.sdkDirectory.get().asFile.absolutePath)
            inputs.dir("../../crates")
            inputs.file("../../Cargo.lock")
            outputs.dir("src/main/jniLibs/$target")
            commandLine(
                cargo, "ndk",
                "-t", target,
                "-o", jniLibs.absolutePath,
                "build", "--release", "-p", "android",
            )
        }
    }

    tasks.register("buildRust") {
        group = "rust"
        description = "Builds crates/android for every ABI"
        dependsOn(targets.map { "buildRust_$it" })
    }

    // Before Android merges the native libraries into the APK.
    tasks.configureEach {
        if (name.startsWith("merge") && name.endsWith("JniLibFolders")) {
            dependsOn("buildRust")
        }
    }
}
