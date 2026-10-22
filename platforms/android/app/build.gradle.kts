plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}
val workspaceVersion = Regex("version = \"([^\"]+)\"").find(rootDir.resolve("../../Cargo.toml").readText())!!.groupValues[1]
val splitApks = providers.gradleProperty("retypeSplitApks").map(String::toBoolean).getOrElse(false)
val validationBuild = providers.gradleProperty("retypeValidationBuild").map(String::toBoolean).getOrElse(false)
val optimizeRelease = providers.gradleProperty("retypeOptimize").map(String::toBoolean).getOrElse(true)
val releaseTests = providers.gradleProperty("retypeTestRelease").map(String::toBoolean).getOrElse(false)
val bundleDictionary by tasks.registering(Copy::class) {
    from(rootDir.resolve("../../NOTICE.txt"))
    from(rootDir.resolve("../../apps/settings-egui/dictionary-packs.json"))
    from(rootDir.resolve("../../LICENSE"))
    from(rootDir.resolve("../../data/dict/raw/wanxiang-base/LICENSE")) { rename { "LICENSE-wanxiang" } }
    from(rootDir.resolve("../../data/dict/raw/LICENSE-Unicode.txt"))
    from(rootDir.resolve("../../data/dict/raw/english")) { include("LICENSE-*") }
    into(layout.buildDirectory.dir("generated/dictionary"))
    doFirst { delete(layout.buildDirectory.file("generated/dictionary/retype-dict.bin")) }
}
val bundleTestResources by tasks.registering(Copy::class) {
    from(rootDir.resolve("build/test-resources")) { include("retype-base.retype-resources") }
    into(layout.buildDirectory.dir("generated/test-resources"))
}
val verifyNative by tasks.registering {
    doLast {
        check(rootDir.resolve("build/native").walkTopDown().any { it.name == "libretype_android.so" }) {
            "Build JNI first: ./build.ps1 (Windows), or cargo ndk -o platforms/android/build/native build -p retype-android --release"
        }
    }
}
android {
    namespace = "io.github.retype.ime"
    compileSdk = 35
    defaultConfig {
        applicationId = "io.github.retype.ime"
        if (validationBuild) applicationIdSuffix = ".validation"
        minSdk = 26
        targetSdk = 35
        versionCode = workspaceVersion.split('.').let { it[0].toInt() * 10000 + it[1].toInt() * 100 + it[2].toInt() }
        versionName = workspaceVersion
        testInstrumentationRunner = if (releaseTests) "io.github.retype.ime.NativeReleaseSmokeRunner" else "androidx.test.runner.AndroidJUnitRunner"
        if (!splitApks) ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }
    buildFeatures { compose = true; buildConfig = true }
    testBuildType = if (releaseTests) "release" else "debug"
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    kotlinOptions { jvmTarget = "17" }
    sourceSets["main"].assets.srcDir(layout.buildDirectory.dir("generated/dictionary"))
    sourceSets["main"].jniLibs.srcDir(rootDir.resolve("build/native"))
    sourceSets["androidTest"].assets.srcDir(layout.buildDirectory.dir("generated/test-resources"))
    packaging { jniLibs { useLegacyPackaging = false } }
    splits { abi {
        isEnable = splitApks
        reset()
        include("arm64-v8a", "x86_64")
        // Previous versions only recognize the universal asset name.
        isUniversalApk = true
    } }
    val signingFile = providers.environmentVariable("RETYPE_ANDROID_KEYSTORE").orNull
    if (!signingFile.isNullOrBlank()) {
        signingConfigs.create("retypeRelease") {
            storeFile = file(signingFile)
            storePassword = providers.environmentVariable("RETYPE_ANDROID_STORE_PASSWORD").get()
            keyAlias = providers.environmentVariable("RETYPE_ANDROID_KEY_ALIAS").get()
            keyPassword = providers.environmentVariable("RETYPE_ANDROID_KEY_PASSWORD").get()
        }
    }
    buildTypes { release {
        isMinifyEnabled = optimizeRelease
        isShrinkResources = optimizeRelease
        proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        testProguardFiles("proguard-test-rules.pro")
        if (!signingFile.isNullOrBlank()) signingConfig = signingConfigs.getByName("retypeRelease")
    } }
}
val requireReleaseSigning by tasks.registering {
    doLast { check(!providers.environmentVariable("RETYPE_ANDROID_KEYSTORE").orNull.isNullOrBlank()) { "Set RETYPE_ANDROID_KEYSTORE and signing credentials before building a release." } }
}
tasks.matching { it.name == "preReleaseBuild" }.configureEach { dependsOn(requireReleaseSigning) }
tasks.named("preBuild") { dependsOn(bundleDictionary, verifyNative) }
tasks.matching { it.name.endsWith("AndroidTestBuild") }.configureEach { dependsOn(bundleTestResources) }
dependencies {
    implementation(platform("androidx.compose:compose-bom:2025.04.01"))
    implementation("androidx.activity:activity-compose:1.10.1")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("androidx.lifecycle:lifecycle-viewmodel:2.8.7")
    implementation("androidx.savedstate:savedstate:1.2.1")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.10.1")
    implementation("androidx.work:work-runtime-ktx:2.10.1")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    androidTestImplementation("androidx.test:runner:1.6.2")
    androidTestImplementation("androidx.test.uiautomator:uiautomator:2.3.0")
}
