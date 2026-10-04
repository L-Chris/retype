plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}
val workspaceVersion = Regex("version = \"([^\"]+)\"").find(rootDir.resolve("../../Cargo.toml").readText())!!.groupValues[1]
val bundleDictionary by tasks.registering(Copy::class) {
    from(rootDir.resolve("../../data/dict/retype-dict.bin"))
    from(rootDir.resolve("../../NOTICE.txt"))
    from(rootDir.resolve("../../apps/settings-egui/dictionary-packs.json"))
    from(rootDir.resolve("../../LICENSE"))
    from(rootDir.resolve("../../data/dict/raw/wanxiang-base/LICENSE")) { rename { "LICENSE-wanxiang" } }
    from(rootDir.resolve("../../data/dict/raw/LICENSE-Unicode.txt"))
    from(rootDir.resolve("../../data/dict/raw/english")) { include("LICENSE-*") }
    into(layout.buildDirectory.dir("generated/dictionary"))
}
val verifyNative by tasks.registering {
    doLast {
        check(rootDir.resolve("../../data/dict/retype-dict.bin").isFile) {
            "Build the offline dictionary first: cargo run --locked --release -p retype-dict-build -- --out data/dict/retype-dict.tsv"
        }
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
        minSdk = 26
        targetSdk = 35
        versionCode = workspaceVersion.split('.').let { it[0].toInt() * 10000 + it[1].toInt() * 100 + it[2].toInt() }
        versionName = workspaceVersion
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }
    buildFeatures { compose = true; buildConfig = true }
    compileOptions { sourceCompatibility = JavaVersion.VERSION_17; targetCompatibility = JavaVersion.VERSION_17 }
    kotlinOptions { jvmTarget = "17" }
    sourceSets["main"].assets.srcDir(layout.buildDirectory.dir("generated/dictionary"))
    sourceSets["main"].jniLibs.srcDir(rootDir.resolve("build/native"))
    packaging { jniLibs { useLegacyPackaging = false } }
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
        isMinifyEnabled = false
        if (!signingFile.isNullOrBlank()) signingConfig = signingConfigs.getByName("retypeRelease")
    } }
}
val requireReleaseSigning by tasks.registering {
    doLast { check(!providers.environmentVariable("RETYPE_ANDROID_KEYSTORE").orNull.isNullOrBlank()) { "Set RETYPE_ANDROID_KEYSTORE and signing credentials before building a release." } }
}
tasks.matching { it.name == "preReleaseBuild" }.configureEach { dependsOn(requireReleaseSigning) }
tasks.named("preBuild") { dependsOn(bundleDictionary, verifyNative) }
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
